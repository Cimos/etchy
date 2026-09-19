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
use serde::Serialize;

#[cfg(feature = "pdf")]
mod pdf;

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

impl Exit {
    /// The 0/1 outcome of a comparison that ran: `passed` (no gated change) is
    /// `NoDiff`, anything else is `DiffFound`.
    fn from_passed(passed: bool) -> Self {
        if passed {
            Exit::NoDiff
        } else {
            Exit::DiffFound
        }
    }
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
    /// checkout). Git mode is explicit — `--git` or a [SUBDIR] — so a mistyped
    /// directory stays a loud "not a directory" error instead of being silently
    /// reinterpreted as a ref.
    #[arg(long)]
    git: bool,
    /// Output format: a terminal summary, machine-readable JSON, or GitHub
    /// Markdown (for a CI step-summary / PR comment).
    #[arg(long, value_enum, default_value_t = Format::Summary)]
    format: Format,
    /// Deprecated alias for `--format json`.
    #[arg(long, conflicts_with = "format")]
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

    /// PDF inputs only: rasterization resolution in DPI (default 150). One DPI
    /// for all sheet sizes — larger sheets produce more pixels, text stays
    /// equally crisp. Higher DPI = crisper diff but more memory/time (per-page
    /// pixel area and the total across both documents are capped; see the error
    /// if you hit it).
    #[arg(long, value_name = "DPI")]
    dpi: Option<f32>,
    /// PDF inputs only: write one overlay PNG per diffed page (`page-<n>.png`)
    /// into this directory (created if it does not exist).
    #[arg(long, value_name = "DIR")]
    out: Option<PathBuf>,
    /// PDF inputs only: hide changed regions smaller than this many connected
    /// pixels from the overlay and tallies as likely anti-aliasing noise
    /// (default 1 = hide nothing). Hidden regions are still reported and still
    /// count as a difference — the floor never causes a silent "no differences".
    /// Raise it to declutter; lower DPI can shrink a real feature to 1px, so 1
    /// is the safe default.
    #[arg(long, value_name = "N")]
    min_region_px: Option<u32>,
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

/// Every kind tag a layer report can carry (`LayerKind::kind_str`). A gate token
/// must match at least one of these — otherwise the filter matches zero layers
/// and the gate silently passes everything, which is how a typo like `coppr`
/// would disarm a CI gate.
const KIND_TAGS: &[&str] = &[
    "top-copper",
    "bottom-copper",
    "inner-copper",
    "top-mask",
    "bottom-mask",
    "top-silk",
    "bottom-silk",
    "top-paste",
    "bottom-paste",
    "drill",
    "drill-pth",
    "drill-npth",
    "outline",
    "documentation",
    "placement",
    "other",
];

impl LayerFilter {
    fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim().to_ascii_lowercase();
        if spec.is_empty() || spec == "all" {
            return Ok(Self {
                all: true,
                tokens: Vec::new(),
            });
        }
        let tokens: Vec<String> = spec
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if tokens.is_empty() {
            anyhow::bail!(
                "--gate-layers has no groups: '{spec}' is only separators (this would \
                 silently disarm the gate). Use 'all' or a comma-separated list of: \
                 copper, mask, silk, paste, drill, outline, documentation/docs, \
                 placement, other"
            );
        }
        for t in &tokens {
            let known = t == "docs" || KIND_TAGS.iter().any(|k| k.contains(t.as_str()));
            if !known {
                anyhow::bail!(
                    "unknown --gate-layers group '{t}' (a typo here would silently disarm \
                     the gate). Valid: all, copper, mask, silk, paste, drill, outline, \
                     documentation/docs, placement, other"
                );
            }
        }
        Ok(Self { all: false, tokens })
    }

    /// Does the gate include a layer with this kind tag (e.g. `top-copper`)?
    fn includes(&self, kind: &str) -> bool {
        self.all
            || self.tokens.iter().any(|t| {
                kind.contains(t.as_str()) || (t == "docs" && kind.contains("documentation"))
            })
    }
}

/// Validate an area threshold read from the command line (#295). clap parses the
/// `f64` with `FromStr`, which accepts `nan`, `inf` and negatives: `area > NaN` is
/// always false (a silently disarmed gate), and `0.0 > -1.0` is true (an identical
/// board fails). A threshold must be finite and >= 0; anything else is a loud
/// exit 2 naming the flag and the value.
fn check_area_threshold(flag: &str, value: Option<f64>) -> Result<Option<f64>> {
    match value {
        Some(t) if !t.is_finite() || t < 0.0 => {
            anyhow::bail!("{flag} must be a finite, non-negative number of mm², got `{t}`")
        }
        other => Ok(other),
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
    fn from_cli(cli: &Cli) -> Result<Self> {
        Ok(Self {
            fail_on_area: check_area_threshold("--fail-on-area", cli.fail_on_area)?,
            fail_on_regions: cli.fail_on_regions,
            filter: LayerFilter::parse(&cli.gate_layers)?,
        })
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

    /// Resolve this gate against a report into the verdict that drives both the
    /// exit code and the printed output — computed once so they cannot disagree.
    fn verdict(&self, report: &DiffReport) -> GateVerdict {
        let (area, regions) = self.totals(report);
        GateVerdict {
            configured: self.configured(),
            passed: !self.fails(report),
            differs: report.any_changes(),
            scope: if self.filter.all {
                "all layers".to_string()
            } else {
                self.tokens_desc()
            },
            changed_area_mm2: area,
            changed_regions: regions,
            fail_on_area: self.fail_on_area,
            fail_on_regions: self.fail_on_regions,
        }
    }
}

/// The gate outcome, resolved against a report: the single source both the exit
/// code and every output format read from (#258). Serializes as the `gate`
/// object in JSON; `differs` is skipped there (it duplicates `any_changes`).
#[derive(Serialize)]
struct GateVerdict {
    /// True when the gate is non-default (a threshold or a layer scope).
    configured: bool,
    /// True ⇔ the process exits 0. With a configured gate this can be true even
    /// when geometry differs (the change was within the gate).
    passed: bool,
    /// Whether any geometry changed at all, regardless of the gate.
    #[serde(skip)]
    differs: bool,
    /// Human description of the gated layer scope (`all layers`, `copper`, …).
    scope: String,
    /// Changed area / region count summed over the gated layers.
    changed_area_mm2: f64,
    changed_regions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    fail_on_area: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fail_on_regions: Option<u32>,
}

impl GateVerdict {
    /// The `result:` line for the terminal summary, reconciled with the exit code
    /// (#258). The confusing case is a *configured* gate that passes while
    /// geometry differs: say so plainly rather than a bare "differences found"
    /// next to an exit 0. Never hides that geometry differs.
    fn summary_result_line(&self) -> String {
        if !self.differs {
            "result: no differences".to_string()
        } else if self.passed {
            // Geometry differs, but the gate downgraded the exit code to 0.
            format!(
                "result: differences found, but within the CI gate — gate PASS, exit 0 \
                 ({}: {:.5} mm², {} regions)",
                self.scope, self.changed_area_mm2, self.changed_regions
            )
        } else {
            "result: differences found".to_string()
        }
    }

    /// Append a one-line gate verdict to the Markdown summary when a gate is
    /// configured, so a CI step-summary shows why the exit code is what it is.
    fn append_markdown_note(&self, s: &mut String) {
        if !self.configured {
            return;
        }
        s.push_str(&format!(
            "\n> **Gate {}** (exit {}) — {}: {:.4} mm², {} regions.\n",
            if self.passed { "PASS" } else { "FAIL" },
            if self.passed { 0 } else { 1 },
            self.scope,
            self.changed_area_mm2,
            self.changed_regions,
        ));
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Validate the gate BEFORE the (expensive) diff, so a bad --gate-layers is a
    // loud, fast exit 2 — never a silently disarmed gate.
    let gate = match Gate::from_cli(&cli) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("etchy: error: {e:#}");
            return Exit::Error.into();
        }
    };
    match run(&cli, &gate) {
        // The CI gate decides the exit code (#M2). Default (no thresholds, all
        // layers) = any change fails, preserving the 0/1 contract. `run` already
        // computed the same verdict for the output, so the message and the exit
        // code can never disagree (#258).
        Ok(RunOutcome::Board { passed: true }) => Exit::NoDiff.into(),
        Ok(RunOutcome::Board { passed: false }) => Exit::DiffFound.into(),
        // The PDF path owns its change verdict (no layer gate; any change = 1).
        Ok(RunOutcome::Pdf { any_changes: true }) => Exit::DiffFound.into(),
        Ok(RunOutcome::Pdf { any_changes: false }) => Exit::NoDiff.into(),
        Err(e) => {
            // Print the full error chain to stderr; map any failure to exit 2.
            eprintln!("etchy: error: {e:#}");
            Exit::Error.into()
        }
    }
}

/// What a run produced: a Gerber board diff (with its gate verdict already
/// applied — `passed` is the 0/1 outcome) or a PDF pixel diff (which has no
/// layers/mm², so it carries only its change verdict).
enum RunOutcome {
    Board { passed: bool },
    Pdf { any_changes: bool },
}

/// Is this input a PDF file? Both signals must agree — `.pdf` extension AND the
/// `%PDF` magic in the first bytes — so a stray directory named `x.pdf` or a
/// mislabelled Gerber never silently reroutes into the pixel-diff path.
fn is_pdf_input(path: &Path) -> bool {
    let ext_is_pdf = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if !ext_is_pdf || !path.is_file() {
        return false;
    }
    let mut magic = [0u8; 4];
    match std::fs::File::open(path) {
        Ok(mut f) => {
            use std::io::Read;
            f.read_exact(&mut magic).is_ok() && &magic == b"%PDF"
        }
        Err(_) => false,
    }
}

/// Why the non-PDF side of a mismatched pair isn't a PDF. `is_pdf_input` folds
/// "missing", "a directory" and "present but not a PDF" into one `false`, which
/// made `etchy real.pdf missing.pdf` claim the missing file "is not a PDF" (#263).
/// Split them so the message names the actual problem.
fn describe_non_pdf(path: &Path) -> &'static str {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "does not exist",
        Err(_) => "is not readable",
        Ok(m) if m.is_dir() => "is a directory, not a PDF",
        // A real, readable file that lacks the `.pdf` extension or the %PDF magic.
        Ok(_) => "is not a PDF",
    }
}

/// The PDF branch, when this build carries it.
#[cfg(feature = "pdf")]
fn run_pdf(cli: &Cli) -> Result<bool> {
    pdf::run_pdf(cli)
}

/// Without the `pdf` feature a PDF input must be a loud, actionable error — not
/// a confusing "not a directory" or a silent skip.
#[cfg(not(feature = "pdf"))]
fn run_pdf(_cli: &Cli) -> Result<bool> {
    anyhow::bail!("this build lacks PDF support — rebuild with --features pdf")
}

fn run(cli: &Cli, gate: &Gate) -> Result<RunOutcome> {
    // Git mode is EXPLICIT: --git, or a [SUBDIR] argument. It must never be
    // inferred from "OLD isn't a directory" — a mistyped folder name that happens
    // to resolve as a ref would silently diff committed revisions the user never
    // asked about (review finding). A typo stays a loud "not a directory" error.
    let git_mode = cli.git || cli.subdir.is_some();

    // PDF inputs dispatch to the parallel pixel-diff path (CLI-6) before any
    // board loading. Both inputs must be PDFs — mixing a PDF with a Gerber
    // directory is a loud error, never a guess. (Git refs are not paths, so the
    // sniff only applies outside git mode.)
    if !git_mode {
        let (old_pdf, new_pdf) = (is_pdf_input(&cli.old), is_pdf_input(&cli.new));
        if old_pdf != new_pdf {
            let (pdf, other) = if old_pdf {
                (&cli.old, &cli.new)
            } else {
                (&cli.new, &cli.old)
            };
            anyhow::bail!(
                "both inputs must be PDFs to run a PDF diff — {} is a PDF but {} {}",
                pdf.display(),
                other.display(),
                describe_non_pdf(other),
            );
        }
        if old_pdf && new_pdf {
            return run_pdf(cli).map(|any_changes| RunOutcome::Pdf { any_changes });
        }
    }
    // The PDF-only flags must not be silently ignored on the geometry path.
    if cli.dpi.is_some() || cli.out.is_some() || cli.min_region_px.is_some() {
        anyhow::bail!("--dpi / --out / --min-region-px apply to PDF inputs only");
    }

    let (old, of, ow, new, nf, nw) = if git_mode {
        let subdir = cli
            .subdir
            .as_deref()
            .and_then(|p| p.to_str())
            .unwrap_or(".");
        if cli.subdir.is_none() {
            // Whole-repo scans flatten every Gerber/Excellon blob at the ref into
            // one board — fine for a single-board repo, garbage when the tree
            // holds several. Say so, loudly, rather than merging boards silently.
            eprintln!(
                "etchy: note: no [SUBDIR] given — diffing every fab file in the whole tree \
                 at each ref; pass a subdirectory to scope the comparison"
            );
        }
        let old_ref = cli.old.to_string_lossy();
        let new_ref = cli.new.to_string_lossy();
        let (old, of, ow) = load_board_git(&old_ref, subdir)
            .with_context(|| format!("loading old revision {old_ref}:{subdir}"))?;
        let (new, nf, nw) = load_board_git(&new_ref, subdir)
            .with_context(|| format!("loading new revision {new_ref}:{subdir}"))?;
        (old, of, ow, new, nf, nw)
    } else {
        let (old, of, ow) = load_board(&cli.old)
            .with_context(|| format!("loading old revision {}", cli.old.display()))?;
        let (new, nf, nw) = load_board(&cli.new)
            .with_context(|| format!("loading new revision {}", cli.new.display()))?;
        (old, of, ow, new, nf, nw)
    };

    let diff = compare_detailed(&old, &new).context("comparing revisions")?;
    let mut report = diff.report.clone();
    // Layer-classification cross-check warnings (#239), old rev then new. A
    // conflict repeated on both revisions is reported once per side, tagged by
    // revision so it's clear which pack the mismatch is in.
    for (rev, ws) in [("old", ow), ("new", nw)] {
        for w in ws {
            report.warnings.push(format!("{rev} {w}"));
        }
    }
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

    // Apply the CI gate once, here, so the printed verdict and the exit code are
    // derived from the same computation (#258): stdout can no longer say
    // "differences found" while the process exits 0 unqualified.
    let verdict = gate.verdict(&report);
    if verdict.configured {
        eprintln!("{}", gate.describe(&report, !verdict.passed));
    }

    // `--json` is the deprecated alias for `--format json`.
    let format = if cli.json { Format::Json } else { cli.format };
    let out = match format {
        Format::Json => board_json(&report, &verdict),
        Format::Md => {
            let mut s = report.to_markdown_summary();
            verdict.append_markdown_note(&mut s);
            s
        }
        Format::Summary => {
            // Warnings go to stderr (as before); the table + result to stdout.
            if !report.warnings.is_empty() {
                eprintln!("\nwarnings:");
                for w in &report.warnings {
                    eprintln!("  - {w}");
                }
            }
            format_summary(&report, &verdict)
        }
    };
    write_stdout(&out, Exit::from_passed(verdict.passed))?;
    Ok(RunOutcome::Board {
        passed: verdict.passed,
    })
}

/// Write a finished report to stdout, treating a downstream pipe that closed
/// early (`etchy … | head`) as a clean exit instead of the panic `println!`
/// raises on a broken pipe (which escaped the 0/1/2 contract as exit 101 — #261).
/// `on_broken_pipe` is the exit code the run has already decided (0 or 1): the
/// consumer going away loses the text, not the verdict, so `etchy … | head` in
/// CI still gates (#296). Any other write failure is a real error and propagates
/// (exit 2), so a genuine I/O problem is never masked. Appends the trailing
/// newline `println!` would.
fn write_stdout(s: &str, on_broken_pipe: Exit) -> Result<()> {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    match lock
        .write_all(s.as_bytes())
        .and_then(|()| lock.write_all(b"\n"))
        .and_then(|()| lock.flush())
    {
        Ok(()) => Ok(()),
        // The consumer went away — there is nothing left to report to. Exit
        // quietly, with the verdict the run reached, not an unconditional 0.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
            std::process::exit(on_broken_pipe as i32);
        }
        Err(e) => Err(e).context("writing to stdout"),
    }
}

/// The Gerber JSON output with the CI-gate verdict attached (#258). The report's
/// own fields are flattened in unchanged (schema unaffected for existing
/// consumers); the added `gate` object lets a JSON consumer read the exit
/// outcome directly — `gate.passed == true` ⇔ exit 0 — instead of inferring it
/// from `any_changes`, which ignores the gate.
fn board_json(report: &DiffReport, verdict: &GateVerdict) -> String {
    #[derive(Serialize)]
    struct BoardJson<'a> {
        #[serde(flatten)]
        report: &'a DiffReport,
        gate: &'a GateVerdict,
    }
    serde_json::to_string_pretty(&BoardJson {
        report,
        gate: verdict,
    })
    .expect("BoardJson serializes")
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
fn board_from_files(
    files: Vec<(String, Vec<u8>)>,
) -> Result<(Board, Option<GerberFormat>, Vec<String>)> {
    let mut layers = Vec::new();
    let mut fmt = None;
    let mut warnings = Vec::new();
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
        let mut kind = etchy_core::classify(stem, ext);
        let mut negative = false;
        // Gerber layer, Excellon/NC drill, pick-and-place, or neither (job file,
        // README) — skip the last. Drill (#62) and P&P (#115) route through their
        // own front-ends so those changes diff instead of being silently dropped.
        let geometry = if etchy_core::looks_like_gerber(&bytes) {
            if fmt.is_none() {
                fmt = etchy_core::gerber_format(&bytes).ok();
            }
            // A negative image (#317): the objects are clearances. The engine swaps
            // added/removed for such a layer so the report still speaks in copper;
            // say so, since the overlay draws the objects as exported.
            if etchy_core::file_polarity(&bytes) == Some(etchy_core::FilePolarity::Negative) {
                negative = true;
                warnings.push(format!(
                    "{name}: negative-polarity image (%TF.FilePolarity,Negative) — \
                     added/removed are reported as material, not as drawn objects"
                ));
            }
            // Cross-check the filename classification against the file's own X2
            // `.FileFunction` attribute (#239): adopt it where the filename was
            // unrecognized, warn on a genuine conflict — never a silent override.
            let (reconciled, warn) =
                etchy_core::reconcile_kind(kind, etchy_core::file_function(&bytes), &name);
            kind = reconciled;
            if let Some(w) = warn {
                warnings.push(w);
            }
            std::sync::Arc::new(
                etchy_core::polygonize_gerber(&bytes)
                    .with_context(|| format!("processing layer {name}"))?,
            )
        } else if etchy_core::looks_like_excellon(&bytes) {
            // Content wins over the filename: an Excellon file named e.g.
            // Board.TXT must land on the Drill layer, not "other" — otherwise
            // `--gate-layers drill` would exclude a real drill change. Plating
            // (PTH/NPTH) still comes from the filename so a misnamed-but-marked
            // file (e.g. `board-NPTH.txt`) keeps its plating (#237).
            kind = etchy_core::LayerKind::Drill(etchy_core::drill_kind(stem));
            std::sync::Arc::new(
                etchy_core::resolve_excellon(&bytes)
                    .with_context(|| format!("processing drill layer {name}"))?,
            )
        } else if etchy_core::looks_like_placement(&bytes) {
            kind = etchy_core::LayerKind::Placement;
            std::sync::Arc::new(
                etchy_core::resolve_placement(&bytes)
                    .with_context(|| format!("processing placement file {name}"))?,
            )
        } else {
            continue;
        };
        layers.push(Layer {
            kind,
            label: name,
            geometry,
            negative,
        });
    }
    Ok((Board { layers }, fmt, warnings))
}

/// Walk a directory (one level) and load it as a revision.
fn load_board(dir: &Path) -> Result<(Board, Option<GerberFormat>, Vec<String>)> {
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    // Every entry must list or the load fails: an entry that errors (transient
    // I/O on a network/FUSE mount) would otherwise vanish from one revision and
    // read as a removed layer, or as nothing at all (#300).
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading directory {}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("listing directory {}", dir.display()))?;
    entries.retain(|p| p.is_file());
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
fn load_board_git(
    gitref: &str,
    subdir: &str,
) -> Result<(Board, Option<GerberFormat>, Vec<String>)> {
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

/// Human-readable summary table (changed layers first), returned as a string so
/// it can flow through the broken-pipe-safe `write_stdout` (#261). Warnings are
/// printed to stderr by the caller. The final `result:` line is reconciled with
/// the gate/exit outcome (#258).
fn format_summary(report: &DiffReport, verdict: &GateVerdict) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<16} {:<13} {:>12} {:>12} {:>9} {:>9}",
        "layer", "status", "added_mm2", "removed_mm2", "+regions", "-regions"
    );
    for l in &report.layers {
        let name = match l.inner_index {
            Some(n) => format!("{}{}", l.kind, n),
            None => l.kind.to_string(),
        };
        let _ = writeln!(
            s,
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
    let _ = writeln!(
        s,
        "\n{} of {} layer(s) changed; total +{:.5} mm² / -{:.5} mm² ({}+/{}- regions)",
        t.layers_changed,
        t.layers_total,
        t.added_area_mm2,
        t.removed_area_mm2,
        t.added_regions,
        t.removed_regions
    );
    s.push_str(&verdict.summary_result_line());
    s
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
    use clap::error::ErrorKind;
    use etchy_core::LayerReport;

    #[test]
    fn deprecated_json_flag_conflicts_with_explicit_format() {
        let err = Cli::try_parse_from(["etchy", "old", "new", "--json", "--format", "md"])
            .expect_err("--json and --format must conflict");
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);

        let cli = Cli::try_parse_from(["etchy", "old", "new", "--json"])
            .expect("--json alone must remain valid");
        assert!(cli.json);
        assert_eq!(cli.format, Format::Summary);
    }

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
            filter: LayerFilter::parse(layers).unwrap(),
        }
    }

    #[test]
    fn filter_copper_matches_all_copper_kinds_only() {
        let f = LayerFilter::parse("copper").unwrap();
        assert!(f.includes("top-copper"));
        assert!(f.includes("inner-copper"));
        assert!(!f.includes("top-silk"));
        assert!(LayerFilter::parse("all").unwrap().includes("top-silk"));
    }

    #[test]
    fn misnamed_drill_file_still_classifies_as_drill() {
        // Altium ships NC drill as Board.TXT — the filename heuristics call it
        // "other", which excluded it from `--gate-layers drill` (review finding:
        // a drilled-hole change passed the gate silently). The content sniff must
        // win: Excellon content ⇒ LayerKind::Drill regardless of name.
        let drl = b"M48\nMETRIC,TZ\nT1C0.500\n%\nT1\nX10.0Y10.0\nM30\n".to_vec();
        let (board, _, _) = board_from_files(vec![("Board.TXT".to_string(), drl)]).unwrap();
        assert_eq!(board.layers.len(), 1);
        assert_eq!(
            board.layers[0].kind,
            etchy_core::LayerKind::Drill(etchy_core::DrillKind::Unspecified)
        );
    }

    #[test]
    fn file_function_recovers_unrecognized_gerber_name() {
        // #239: a Gerber whose filename matches no naming pattern (→ Other) but
        // whose X2 .FileFunction declares it top copper must be classified from the
        // attribute, not left in the wrong bucket. A warning surfaces the recovery.
        let g = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,Copper,L1,Top*%\n\
                  %ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warns) =
            board_from_files(vec![("mystery-layer.xyz".to_string(), g)]).unwrap();
        assert_eq!(board.layers.len(), 1);
        assert_eq!(board.layers[0].kind, etchy_core::LayerKind::TopCopper);
        assert_eq!(warns.len(), 1, "the promotion must be surfaced");
        assert!(warns[0].contains("mystery-layer.xyz") && warns[0].contains("top-copper"));
    }

    #[test]
    fn file_function_conflict_keeps_filename_and_warns() {
        // #239 trust bar: filename says top copper, attribute says bottom — a real
        // conflict. The filename classification is retained (pairing-stable) and the
        // disagreement is warned, never silently resolved.
        let g = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,Copper,L4,Bot*%\n\
                  %ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warns) = board_from_files(vec![("board-F_Cu.gbr".to_string(), g)]).unwrap();
        assert_eq!(board.layers[0].kind, etchy_core::LayerKind::TopCopper);
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("top-copper") && warns[0].contains("bottom-copper"));
    }

    #[test]
    fn negative_file_polarity_tags_the_layer_and_warns() {
        // #317: %TF.FilePolarity,Negative marks the layer so the engine swaps
        // added/removed, and the loader says so (the overlay draws the objects as
        // exported). A positive or unattributed file stays untagged and silent.
        let neg = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FilePolarity,Negative*%\n\
                    %ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warns) =
            board_from_files(vec![("board-In1_Cu.gbr".to_string(), neg)]).unwrap();
        assert!(board.layers[0].negative);
        assert_eq!(warns.len(), 1);
        assert!(warns[0].contains("board-In1_Cu.gbr") && warns[0].contains("Negative"));

        let pos = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FilePolarity,Positive*%\n\
                    %ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warns) =
            board_from_files(vec![("board-In1_Cu.gbr".to_string(), pos)]).unwrap();
        assert!(!board.layers[0].negative);
        assert!(warns.is_empty(), "positive must not warn: {warns:?}");
    }

    #[test]
    fn file_function_agreement_is_silent() {
        // A confirming attribute must not produce noise.
        let g = b"%FSLAX46Y46*%\n%MOMM*%\n%TF.FileFunction,Copper,L1,Top*%\n\
                  %ADD10C,0.5*%\nD10*\nX0Y0D03*\nM02*\n"
            .to_vec();
        let (board, _, warns) = board_from_files(vec![("board-F_Cu.gbr".to_string(), g)]).unwrap();
        assert_eq!(board.layers[0].kind, etchy_core::LayerKind::TopCopper);
        assert!(warns.is_empty(), "agreement must not warn: {warns:?}");
    }

    #[test]
    fn filter_rejects_unknown_tokens() {
        // A typo ('coppr') used to build a filter matching ZERO layers, so the
        // gate passed every diff — a silently disarmed CI gate (review finding).
        assert!(LayerFilter::parse("coppr").is_err());
        assert!(LayerFilter::parse("copper,silkscreen-typo").is_err());
        // Known groups and aliases still parse.
        for ok in [
            "copper",
            "mask",
            "silk",
            "paste",
            "drill",
            "outline",
            "docs",
            "documentation",
            "placement",
            "other",
            "copper,silk",
            "all",
            "",
        ] {
            assert!(LayerFilter::parse(ok).is_ok(), "'{ok}' should be valid");
        }
    }

    #[test]
    fn area_threshold_rejects_nan_inf_and_negative() {
        // #295: `--fail-on-area nan` disarmed the gate (area > NaN is never true)
        // and a negative threshold failed an identical board (0.0 > -1.0). Every
        // area threshold must be finite and >= 0, refused before the diff runs.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
            let err = check_area_threshold("--fail-on-area", Some(bad))
                .err()
                .unwrap_or_else(|| panic!("{bad} must be rejected"));
            let msg = err.to_string();
            assert!(msg.contains("--fail-on-area"), "names the flag: {msg}");
            assert!(msg.contains(&bad.to_string()), "names the value: {msg}");
        }
        // Zero (any change fails) and an ordinary threshold are accepted as-is.
        assert_eq!(
            check_area_threshold("--fail-on-area", Some(0.0)).unwrap(),
            Some(0.0)
        );
        assert_eq!(
            check_area_threshold("--fail-on-area", Some(2.5)).unwrap(),
            Some(2.5)
        );
        assert_eq!(check_area_threshold("--fail-on-area", None).unwrap(), None);
    }

    #[test]
    fn gate_from_cli_refuses_nan_area() {
        // The real clap path: `nan` parses as an f64, so the check must sit in
        // Gate::from_cli, which main() runs before the diff and maps to exit 2.
        let cli = Cli::try_parse_from(["etchy", "--fail-on-area", "nan", "old", "new"]).unwrap();
        let err = Gate::from_cli(&cli).err().expect("nan must be refused");
        assert!(err.to_string().contains("--fail-on-area"));
        let cli = Cli::try_parse_from(["etchy", "--fail-on-area", "2.5", "old", "new"]).unwrap();
        assert_eq!(Gate::from_cli(&cli).unwrap().fail_on_area, Some(2.5));
    }

    #[test]
    fn filter_rejects_separator_only_spec() {
        // "," passed the ""/"all" test, then split to ZERO tokens, so the
        // validation loop never ran and the filter matched no layer at all —
        // `--gate-layers ,` exited 0 on a real change (#294).
        for bad in [",", " , ", ",,", " ,, "] {
            assert!(
                LayerFilter::parse(bad).is_err(),
                "'{bad}' should be rejected"
            );
        }
        // "" and "all" still mean every layer.
        for all in ["", "all", " all "] {
            let f = LayerFilter::parse(all).unwrap();
            assert!(f.all, "'{all}' should mean all layers");
            assert!(f.includes("top-copper"));
        }
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

    #[test]
    fn passing_gate_summary_does_not_contradict_exit_0() {
        // #258: geometry differs (total ~0.52 mm²) but a 1.0 mm² gate passes, so
        // the process exits 0. The summary must NOT read as a bare "differences
        // found" beside that exit 0 — it must say the change was within the gate,
        // while still admitting geometry differs.
        let r = report();
        let v = gate(Some(1.0), None, "all").verdict(&r);
        assert!(v.passed, "the change is within the 1.0 mm² gate");
        let summary = format_summary(&r, &v);
        assert!(summary.contains("differences found"), "still admits a diff");
        assert!(
            summary.contains("gate PASS") && summary.contains("exit 0"),
            "reconciles with the exit code: {summary}"
        );
    }

    #[test]
    fn failing_gate_summary_stays_plain() {
        // A gate that fails (or the default gate) keeps the plain wording — it
        // matches exit 1, so there is nothing to reconcile.
        let r = report();
        let v = gate(None, None, "all").verdict(&r);
        assert!(!v.passed);
        let summary = format_summary(&r, &v);
        assert!(summary.contains("result: differences found"));
        assert!(
            !summary.contains("gate PASS"),
            "no downgrade note: {summary}"
        );
    }

    #[test]
    fn no_change_summary_reads_no_differences() {
        let r = DiffReport::new(vec![layer("top-copper", 0.0, 0)], Vec::new());
        let v = gate(None, None, "all").verdict(&r);
        assert!(v.passed);
        assert!(format_summary(&r, &v).contains("result: no differences"));
    }

    #[test]
    fn json_carries_the_gate_verdict() {
        // #258: a JSON consumer must be able to read the exit outcome directly.
        let r = report();
        let v = gate(Some(1.0), None, "all").verdict(&r);
        let json = board_json(&r, &v);
        // The report's own contract is untouched (flattened in).
        assert!(json.contains("\"schema_version\": 1"), "json: {json}");
        assert!(json.contains("\"any_changes\": true"));
        // …and the gate verdict is attached, matching the exit code.
        assert!(json.contains("\"gate\""));
        assert!(json.contains("\"configured\": true"));
        assert!(json.contains("\"passed\": true"));
        assert!(json.contains("\"fail_on_area\": 1.0"));
    }

    #[test]
    fn load_board_lists_every_file_in_sorted_order() {
        // #300: the directory listing must be complete and deterministic. The
        // erroring-`DirEntry` path itself can't be provoked portably, so this
        // pins the two properties the listing code is responsible for.
        const GERBER: &str = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,1.0*%\nD10*\nX0Y0D03*\nM02*\n";
        let dir = std::env::temp_dir().join(format!("etchy-300-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Written out of order so a creation-order or readdir-order listing
        // would not happen to match.
        for name in ["c-B_Cu.gbl", "a-F_Cu.gtl", "b-F_Mask.gts"] {
            std::fs::write(dir.join(name), GERBER).unwrap();
        }
        let (board, _, _) = load_board(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let labels: Vec<&str> = board.layers.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["a-F_Cu.gtl", "b-F_Mask.gts", "c-B_Cu.gbl"]);
    }
}
