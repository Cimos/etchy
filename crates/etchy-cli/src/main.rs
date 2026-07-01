//! etchy — fast, trustworthy PCB visual + geometric diff (CLI; primary surface).
//!
//! This binary is the **I/O shell**: it walks directories, reads files, classifies
//! filenames → [`LayerKind`], renders output, and owns the exit-code contract. All
//! geometry logic lives in the pure `etchy-core` engine (CLAUDE.md: core has no
//! I/O policy). M1 slice: flash-only Gerber; unsupported features fail loud.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use etchy_core::{
    compare_detailed, coordinate_mismatch_warning, layer_svg, Board, DiffReport, GerberFormat,
    Layer, LayerStatus,
};

/// etchy's CI exit-code contract.
#[repr(i32)]
enum Exit {
    /// No differences found.
    NoDiff = 0,
    /// Differences found (CI gating).
    DiffFound = 1,
    /// An error prevented a comparison.
    Error = 2,
}

impl From<Exit> for ExitCode {
    fn from(code: Exit) -> Self {
        ExitCode::from(code as u8)
    }
}

/// Fast, trustworthy PCB visual + geometric diff. Point it at two revisions of a
/// board's Gerber output and it shows — and measures — exactly what changed.
#[derive(Parser, Debug)]
#[command(name = "etchy", version, about)]
struct Cli {
    /// Old revision: a directory of Gerber/Excellon files, or a git ref (see
    /// `--git` / the optional [SUBDIR] argument).
    old: PathBuf,
    /// New revision: a directory, or a git ref.
    new: PathBuf,
    /// Optional: with git refs, the subdirectory in the repo where the fab files
    /// live (default: the repo root). Its presence implies `--git`.
    subdir: Option<PathBuf>,
    /// Treat OLD and NEW as git refs and read the committed Gerbers at each (no
    /// checkout). Auto-enabled when OLD is not an existing directory.
    #[arg(long)]
    git: bool,
    /// Output format: a terminal summary, machine-readable JSON, or GitHub
    /// Markdown (for a CI step-summary / PR comment).
    #[arg(long, value_enum, default_value_t = Format::Summary)]
    format: Format,
    /// Deprecated alias for `--format json`.
    #[arg(long)]
    json: bool,
    /// Write a per-layer SVG of the diff into this directory: one `<layer>.svg`
    /// per changed layer (base faint grey, removed red, added green). The
    /// directory is created if it does not exist.
    #[arg(long, value_name = "DIR")]
    svg: Option<PathBuf>,
    /// Write a single self-contained HTML report to this file: totals, warnings,
    /// and each changed layer's overlay inlined (no external assets).
    #[arg(long, value_name = "FILE")]
    html: Option<PathBuf>,

    /// CI gate: fail (exit 1) only when the changed area on the gated layers
    /// exceeds this many mm². Omitted ⇒ any change on the gated layers fails.
    #[arg(long, value_name = "MM2")]
    fail_on_area: Option<f64>,
    /// CI gate: fail (exit 1) only when the changed region count on the gated
    /// layers exceeds this. Omitted ⇒ any change fails (unless --fail-on-area is
    /// set); if both are set, either being exceeded fails.
    #[arg(long, value_name = "N")]
    fail_on_regions: Option<u32>,
    /// Which layers the CI gate considers: `all` (default) or a comma list of
    /// groups — copper, mask, silk, paste, drill, outline, documentation, other.
    /// E.g. `--gate-layers copper` fails on copper changes and ignores silkscreen.
    #[arg(long, value_name = "SPEC", default_value = "all")]
    gate_layers: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Summary,
    Json,
    Md,
}

/// Which layer kinds the CI gate counts, from `--gate-layers` (#M2). `all`, or a
/// comma list of group words matched against the kebab-case kind tag — so
/// `copper` covers top/bottom/inner copper, `silk` covers both silkscreens, etc.
struct LayerFilter {
    all: bool,
    tokens: Vec<String>,
}

impl LayerFilter {
    fn parse(spec: &str) -> Self {
        let spec = spec.trim().to_ascii_lowercase();
        if spec.is_empty() || spec == "all" {
            return Self {
                all: true,
                tokens: Vec::new(),
            };
        }
        let tokens = spec
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self { all: false, tokens }
    }

    /// Does the gate include a layer with this kind tag (e.g. `top-copper`)?
    fn includes(&self, kind: &str) -> bool {
        self.all
            || self.tokens.iter().any(|t| {
                kind.contains(t.as_str()) || (t == "docs" && kind.contains("documentation"))
            })
    }
}

/// The CI gate (#M2): decides whether a diff should fail (exit 1). Thresholds are
/// opt-in — with none set, any change on the gated layers fails (so
/// `--gate-layers copper` alone means "fail on copper, ignore silkscreen").
struct Gate {
    fail_on_area: Option<f64>,
    fail_on_regions: Option<u32>,
    filter: LayerFilter,
}

impl Gate {
    fn from_cli(cli: &Cli) -> Self {
        Self {
            fail_on_area: cli.fail_on_area,
            fail_on_regions: cli.fail_on_regions,
            filter: LayerFilter::parse(&cli.gate_layers),
        }
    }

    /// (changed area mm², changed region count) summed over the gated layers.
    fn totals(&self, report: &DiffReport) -> (f64, u32) {
        report
            .layers
            .iter()
            .filter(|l| self.filter.includes(l.kind))
            .fold((0.0, 0), |(a, r), l| {
                (
                    a + l.added_area_mm2 + l.removed_area_mm2,
                    r + l.added_regions + l.removed_regions,
                )
            })
    }

    /// Should this diff fail the gate (exit 1)?
    fn fails(&self, report: &DiffReport) -> bool {
        let (area, regions) = self.totals(report);
        match (self.fail_on_area, self.fail_on_regions) {
            // No threshold set → any change on the gated layers fails.
            (None, None) => area > 0.0 || regions > 0,
            (a, r) => a.is_some_and(|t| area > t) || r.is_some_and(|t| regions > t),
        }
    }

    /// True when the gate is non-default (a threshold or a layer scope), so the
    /// verdict is worth surfacing in the output.
    fn configured(&self) -> bool {
        self.fail_on_area.is_some() || self.fail_on_regions.is_some() || !self.filter.all
    }

    /// A one-line human verdict for stderr.
    fn describe(&self, report: &DiffReport, fails: bool) -> String {
        let (area, regions) = self.totals(report);
        let scope = if self.filter.all {
            "all layers".to_string()
        } else {
            self.tokens_desc()
        };
        format!(
            "etchy: gate {} — {}: changed {:.4} mm², {} regions",
            if fails { "FAIL" } else { "PASS" },
            scope,
            area,
            regions,
        )
    }

    fn tokens_desc(&self) -> String {
        self.filter.tokens.join(",")
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(report) => {
            // The CI gate decides the exit code (#M2). Default (no thresholds,
            // all layers) = any change fails, preserving the 0/1 contract.
            let gate = Gate::from_cli(&cli);
            let fails = gate.fails(&report);
            if gate.configured() {
                eprintln!("{}", gate.describe(&report, fails));
            }
            if fails {
                Exit::DiffFound.into()
            } else {
                Exit::NoDiff.into()
            }
        }
        Err(e) => {
            // Print the full error chain to stderr; map any failure to exit 2.
            eprintln!("etchy: error: {e:#}");
            Exit::Error.into()
        }
    }
}

fn run(cli: &Cli) -> Result<DiffReport> {
    // Git mode when asked (--git), when a [SUBDIR] is given, or auto when OLD is
    // not an existing directory (so `etchy v1 v2` "just works" in a repo).
    let git_mode = cli.git || cli.subdir.is_some() || !cli.old.is_dir();
    let (old, of, new, nf) = if git_mode {
        let subdir = cli
            .subdir
            .as_deref()
            .and_then(|p| p.to_str())
            .unwrap_or(".");
        let old_ref = cli.old.to_string_lossy();
        let new_ref = cli.new.to_string_lossy();
        let (old, of) = load_board_git(&old_ref, subdir)
            .with_context(|| format!("loading old revision {old_ref}:{subdir}"))?;
        let (new, nf) = load_board_git(&new_ref, subdir)
            .with_context(|| format!("loading new revision {new_ref}:{subdir}"))?;
        (old, of, new, nf)
    } else {
        let (old, of) = load_board(&cli.old)
            .with_context(|| format!("loading old revision {}", cli.old.display()))?;
        let (new, nf) = load_board(&cli.new)
            .with_context(|| format!("loading new revision {}", cli.new.display()))?;
        (old, of, new, nf)
    };

    let diff = compare_detailed(&old, &new).context("comparing revisions")?;
    let mut report = diff.report.clone();
    if let (Some(o), Some(n)) = (of, nf) {
        if let Some(w) = coordinate_mismatch_warning(&o, &n) {
            report.warnings.push(w);
        }
    }

    // Optional SVG export: one file per changed layer (headless render).
    if let Some(dir) = &cli.svg {
        write_svgs(&diff, dir).with_context(|| format!("writing SVGs to {}", dir.display()))?;
    }

    // Optional self-contained HTML report (M1 output).
    if let Some(path) = &cli.html {
        let html = etchy_core::board_report_html(
            &diff,
            &cli.old.to_string_lossy(),
            &cli.new.to_string_lossy(),
        );
        std::fs::write(path, html)
            .with_context(|| format!("writing HTML report to {}", path.display()))?;
    }

    // `--json` is the deprecated alias for `--format json`.
    let format = if cli.json { Format::Json } else { cli.format };
    match format {
        Format::Json => println!("{}", report.to_json_pretty()),
        Format::Md => println!("{}", report.to_markdown_summary()),
        Format::Summary => print_summary(&report),
    }
    Ok(report)
}

/// Reject any single layer file larger than this before reading it into RAM. The
/// loaders copy the bytes a few times (read → utf8 → normalized), so an oversized
/// or junk file is a quick OOM (#82). 100 MiB is far above any real fab layer.
const MAX_LAYER_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Walk a directory (one level), read each Gerber file, classify it, and
/// polygonize it into a [`Layer`]. I/O + path/naming policy live here, not in core.
/// Build a board from an in-memory set of `(filename, bytes)` — the shared
/// classify + route (Gerber / Excellon / skip) behind both directory and git-ref
/// loading (#93). Non-Gerber, non-Excellon files are skipped.
fn board_from_files(files: Vec<(String, Vec<u8>)>) -> Result<(Board, Option<GerberFormat>)> {
    let mut layers = Vec::new();
    let mut fmt = None;
    for (name, bytes) in files {
        if bytes.len() as u64 > MAX_LAYER_FILE_BYTES {
            anyhow::bail!(
                "{name} is {} bytes, over the {MAX_LAYER_FILE_BYTES}-byte per-file limit",
                bytes.len()
            );
        }
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (name.as_str(), ""),
        };
        let kind = etchy_core::classify(stem, ext);
        // Gerber layer, Excellon/NC drill, or neither (job file, README) — skip the
        // last, but route drill files through the Excellon front-end (#62) so drill
        // changes actually diff instead of being silently dropped.
        let geometry = if etchy_core::looks_like_gerber(&bytes) {
            if fmt.is_none() {
                fmt = etchy_core::gerber_format(&bytes).ok();
            }
            std::sync::Arc::new(
                etchy_core::polygonize_gerber(&bytes)
                    .with_context(|| format!("processing layer {name}"))?,
            )
        } else if etchy_core::looks_like_excellon(&bytes) {
            std::sync::Arc::new(
                etchy_core::resolve_excellon(&bytes)
                    .with_context(|| format!("processing drill layer {name}"))?,
            )
        } else {
            continue;
        };
        layers.push(Layer {
            kind,
            label: name,
            geometry,
        });
    }
    Ok((Board { layers }, fmt))
}

/// Walk a directory (one level) and load it as a revision.
fn load_board(dir: &Path) -> Result<(Board, Option<GerberFormat>)> {
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    entries.sort();

    let mut files = Vec::with_capacity(entries.len());
    for path in entries {
        let len = path
            .metadata()
            .with_context(|| format!("reading metadata for {}", path.display()))?
            .len();
        if len > MAX_LAYER_FILE_BYTES {
            anyhow::bail!(
                "{} is {len} bytes, over the {MAX_LAYER_FILE_BYTES}-byte per-file limit",
                path.display()
            );
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        files.push((name, bytes));
    }
    board_from_files(files)
}

/// Load a revision straight from a git ref — `etchy <refA> <refB> [subdir]` over a
/// repo of committed Gerbers, no checkout (#M2). Lists the blobs at
/// `<ref>:<subdir>` via `git ls-tree` and reads each with `git show`.
fn load_board_git(gitref: &str, subdir: &str) -> Result<(Board, Option<GerberFormat>)> {
    let listing = git_stdout(&["ls-tree", "-r", "-z", "--name-only", gitref, "--", subdir])
        .with_context(|| format!("listing gerbers at {gitref}:{subdir}"))?;
    let paths: Vec<&str> = listing.split('\0').filter(|s| !s.is_empty()).collect();
    if paths.is_empty() {
        anyhow::bail!("no files found at git ref {gitref}:{subdir}");
    }
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = git_bytes(&["show", &format!("{gitref}:{path}")])
            .with_context(|| format!("reading {gitref}:{path}"))?;
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
        files.push((name, bytes));
    }
    board_from_files(files)
}

/// Run `git <args>` and return its stdout as text, failing loud on a non-zero exit.
fn git_stdout(args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_bytes(args)?).into_owned())
}

/// Run `git <args>` and return its raw stdout bytes (Gerber/Excellon are ASCII,
/// but blobs are read as bytes so nothing is mangled).
fn git_bytes(args: &[&str]) -> Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .context("running git (is it installed and are you inside the repo?)")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Write one SVG per *changed* layer into `dir` (created if missing). Filenames
/// are the layer's display name (e.g. `top-copper.svg`); the geometry → SVG
/// rendering itself is the pure `etchy_core::layer_svg`.
fn write_svgs(diff: &etchy_core::BoardDiff, dir: &Path) -> Result<usize> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating output directory {}", dir.display()))?;
    let mut written = 0;
    for (i, layer) in diff.layers.iter().enumerate() {
        if layer.status == LayerStatus::Unchanged {
            continue;
        }
        let svg = layer_svg(layer);
        // Prefix with the stack index so two layers that share a display name
        // (e.g. two `other` layers) never clobber each other's file.
        let path = dir.join(format!("{i:02}-{}.svg", layer.name()));
        std::fs::write(&path, svg).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("etchy: wrote {}", path.display());
        written += 1;
    }
    Ok(written)
}

/// Human-readable summary table to stdout (changed layers first).
fn print_summary(report: &DiffReport) {
    println!(
        "{:<16} {:<13} {:>12} {:>12} {:>9} {:>9}",
        "layer", "status", "added_mm2", "removed_mm2", "+regions", "-regions"
    );
    for l in &report.layers {
        let name = match l.inner_index {
            Some(n) => format!("{}{}", l.kind, n),
            None => l.kind.to_string(),
        };
        println!(
            "{:<16} {:<13} {:>12.5} {:>12.5} {:>9} {:>9}",
            name,
            status_str(l.status),
            l.added_area_mm2,
            l.removed_area_mm2,
            l.added_regions,
            l.removed_regions
        );
    }
    let t = &report.totals;
    println!(
        "\n{} of {} layer(s) changed; total +{:.5} mm² / -{:.5} mm² ({}+/{}- regions)",
        t.layers_changed,
        t.layers_total,
        t.added_area_mm2,
        t.removed_area_mm2,
        t.added_regions,
        t.removed_regions
    );
    if !report.warnings.is_empty() {
        eprintln!("\nwarnings:");
        for w in &report.warnings {
            eprintln!("  - {w}");
        }
    }
    println!(
        "{}",
        if report.any_changes() {
            "result: differences found"
        } else {
            "result: no differences"
        }
    );
}

fn status_str(s: etchy_core::LayerStatus) -> &'static str {
    use etchy_core::LayerStatus::*;
    match s {
        Unchanged => "unchanged",
        Changed => "changed",
        AddedLayer => "added-layer",
        RemovedLayer => "removed-layer",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etchy_core::LayerReport;

    fn layer(kind: &'static str, area_mm2: f64, regions: u32) -> LayerReport {
        LayerReport {
            kind,
            inner_index: None,
            label_old: None,
            label_new: Some("x".into()),
            status: if area_mm2 > 0.0 || regions > 0 {
                LayerStatus::Changed
            } else {
                LayerStatus::Unchanged
            },
            added_area_mm2: area_mm2,
            removed_area_mm2: 0.0,
            added_area_nm2: "0".into(),
            removed_area_nm2: "0".into(),
            added_regions: regions,
            removed_regions: 0,
        }
    }

    // A copper change + a small silk change.
    fn report() -> DiffReport {
        DiffReport::new(
            vec![
                layer("top-copper", 0.5, 3),
                layer("top-silk", 0.02, 1),
                layer("bottom-mask", 0.0, 0),
            ],
            Vec::new(),
        )
    }

    fn gate(area: Option<f64>, regions: Option<u32>, layers: &str) -> Gate {
        Gate {
            fail_on_area: area,
            fail_on_regions: regions,
            filter: LayerFilter::parse(layers),
        }
    }

    #[test]
    fn filter_copper_matches_all_copper_kinds_only() {
        let f = LayerFilter::parse("copper");
        assert!(f.includes("top-copper"));
        assert!(f.includes("inner-copper"));
        assert!(!f.includes("top-silk"));
        assert!(LayerFilter::parse("all").includes("top-silk"));
    }

    #[test]
    fn default_gate_fails_on_any_change() {
        // No thresholds, all layers → any change fails (the 0/1 contract).
        assert!(gate(None, None, "all").fails(&report()));
    }

    #[test]
    fn area_threshold_ignores_small_changes() {
        // Total changed area is 0.52 mm²; a 1.0 mm² threshold passes it.
        assert!(!gate(Some(1.0), None, "all").fails(&report()));
        // …but 0.1 mm² fails.
        assert!(gate(Some(0.1), None, "all").fails(&report()));
    }

    #[test]
    fn scope_to_copper_ignores_silk() {
        // Only copper counts; a copper-clean board with silk churn passes.
        let silk_only = DiffReport::new(vec![layer("top-silk", 0.3, 2)], Vec::new());
        assert!(!gate(None, None, "copper").fails(&silk_only));
        // Copper changed → fails when scoped to copper.
        assert!(gate(None, None, "copper").fails(&report()));
    }

    #[test]
    fn region_threshold_gates_on_count() {
        // Copper has 3 changed regions; threshold 5 passes, 2 fails.
        assert!(!gate(None, Some(5), "copper").fails(&report()));
        assert!(gate(None, Some(2), "copper").fails(&report()));
    }
}
