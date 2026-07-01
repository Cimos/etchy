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
    /// Old revision: a directory of Gerber files.
    old: PathBuf,
    /// New revision: a directory of Gerber files.
    new: PathBuf,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Format {
    Summary,
    Json,
    Md,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(report) => {
            if report.any_changes() {
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
    let (old, of) = load_board(&cli.old)
        .with_context(|| format!("loading old revision {}", cli.old.display()))?;
    let (new, nf) = load_board(&cli.new)
        .with_context(|| format!("loading new revision {}", cli.new.display()))?;

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

    let mut layers = Vec::new();
    let mut fmt = None;
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
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let kind = etchy_core::classify(stem, ext);
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_string();
        // Gerber layer, Excellon/NC drill, or neither (job file, README) — skip the
        // last, but route drill files through the Excellon front-end (#62) so drill
        // changes actually diff instead of being silently dropped.
        let geometry = if etchy_core::looks_like_gerber(&bytes) {
            if fmt.is_none() {
                fmt = etchy_core::gerber_format(&bytes).ok();
            }
            std::sync::Arc::new(
                etchy_core::polygonize_gerber(&bytes)
                    .with_context(|| format!("processing layer {label}"))?,
            )
        } else if etchy_core::looks_like_excellon(&bytes) {
            std::sync::Arc::new(
                etchy_core::resolve_excellon(&bytes)
                    .with_context(|| format!("processing drill layer {label}"))?,
            )
        } else {
            continue;
        };
        layers.push(Layer {
            kind,
            label,
            geometry,
        });
    }
    Ok((Board { layers }, fmt))
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
