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
use etchy_core::{compare, Board, DiffReport, Layer, LayerKind};

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
    /// Emit the machine-readable JSON report to stdout instead of a summary.
    #[arg(long)]
    json: bool,
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
    let old = load_board(&cli.old)
        .with_context(|| format!("loading old revision {}", cli.old.display()))?;
    let new = load_board(&cli.new)
        .with_context(|| format!("loading new revision {}", cli.new.display()))?;

    let report = compare(&old, &new).context("comparing revisions")?;

    if cli.json {
        println!("{}", report.to_json_pretty());
    } else {
        print_summary(&report);
    }
    Ok(report)
}

/// Walk a directory (one level), read each Gerber file, classify it, and
/// polygonize it into a [`Layer`]. I/O + path/naming policy live here, not in core.
fn load_board(dir: &Path) -> Result<Board> {
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
    for path in entries {
        let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        if !looks_like_gerber(&bytes) {
            continue; // not a Gerber layer (e.g. drill, job file) — Excellon is a later increment
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let kind = classify(stem, ext);
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(stem)
            .to_string();
        let geometry = etchy_core::polygonize_gerber(&bytes)
            .with_context(|| format!("processing layer {label}"))?;
        layers.push(Layer {
            kind,
            label,
            geometry,
        });
    }
    Ok(Board { layers })
}

/// Content sniff for a Gerber layer. RS-274X requires a format-spec (`%FS`) and a
/// mode (`%MO`) statement, each its own `%…%` block on a line. We look for either
/// as a **line-start** marker, scanning the **whole** file — not a byte-capped
/// substring match (which both skipped real layers with long X2/attribute headers
/// and false-matched prose containing `%MO`). Non-Gerber files (drill, job,
/// READMEs) don't carry these and are skipped; Excellon is a later increment.
fn looks_like_gerber(bytes: &[u8]) -> bool {
    String::from_utf8_lossy(bytes).lines().any(|l| {
        let t = l.trim_start();
        t.starts_with("%FS") || t.starts_with("%MO")
    })
}

/// Map a Gerber filename → [`LayerKind`] (naming policy — deliberately in the CLI,
/// not the engine). Rename-tolerant: keys on KiCad-style suffixes and on the
/// standard Gerber extensions.
fn classify(stem: &str, ext: &str) -> LayerKind {
    let s = stem.to_ascii_uppercase();
    let e = ext.to_ascii_lowercase();

    if let Some(n) = inner_copper_index(&s) {
        return LayerKind::InnerCopper(n);
    }
    let has = |needle: &str| s.contains(needle);
    if has("F_CU") || has("F.CU") || e == "gtl" {
        LayerKind::TopCopper
    } else if has("B_CU") || has("B.CU") || e == "gbl" {
        LayerKind::BottomCopper
    } else if has("F_MASK") || has("F.MASK") || e == "gts" {
        LayerKind::TopMask
    } else if has("B_MASK") || has("B.MASK") || e == "gbs" {
        LayerKind::BottomMask
    } else if has("F_SILK") || has("F.SILK") || e == "gto" {
        LayerKind::TopSilk
    } else if has("B_SILK") || has("B.SILK") || e == "gbo" {
        LayerKind::BottomSilk
    } else if has("F_PASTE") || has("F.PASTE") || e == "gtp" {
        LayerKind::TopPaste
    } else if has("B_PASTE") || has("B.PASTE") || e == "gbp" {
        LayerKind::BottomPaste
    } else if has("DRILL") || e == "drl" || e == "xln" {
        LayerKind::Drill
    } else if has("EDGE") || has("OUTLINE") || e == "gko" || e == "gm1" {
        LayerKind::Outline
    } else {
        LayerKind::Other
    }
}

/// Extract the inner-copper index from an uppercased stem: `IN<digits>` followed
/// by `_CU` / `.CU` (e.g. `In3_Cu` → 3). Returns `None` otherwise.
fn inner_copper_index(upper: &str) -> Option<u8> {
    let bytes = upper.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if &bytes[i..i + 2] == b"IN" {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 2 {
                let rest = &upper[j..];
                if rest.starts_with("_CU") || rest.starts_with(".CU") {
                    if let Ok(n) = upper[i + 2..j].parse::<u8>() {
                        return Some(n);
                    }
                }
            }
        }
        i += 1;
    }
    None
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

    #[test]
    fn classify_kicad_names() {
        assert_eq!(classify("synth-F_Cu", "gbr"), LayerKind::TopCopper);
        assert_eq!(classify("synth-B_Cu", "gbr"), LayerKind::BottomCopper);
        assert_eq!(classify("synth-In1_Cu", "gbr"), LayerKind::InnerCopper(1));
        assert_eq!(classify("synth-In12_Cu", "gbr"), LayerKind::InnerCopper(12));
        assert_eq!(classify("synth-F_Mask", "gbr"), LayerKind::TopMask);
        assert_eq!(classify("synth-F_Silk", "gbr"), LayerKind::TopSilk);
        assert_eq!(classify("synth-B_Paste", "gbr"), LayerKind::BottomPaste);
        assert_eq!(classify("board-Edge_Cuts", "gm1"), LayerKind::Outline);
        assert_eq!(classify("something", "gtl"), LayerKind::TopCopper);
        assert_eq!(classify("random", "txt"), LayerKind::Other);
    }

    #[test]
    fn inner_index_does_not_false_match() {
        assert_eq!(inner_copper_index("OUTLINE"), None);
        assert_eq!(inner_copper_index("F_CU"), None);
        assert_eq!(inner_copper_index("IN3_CU"), Some(3));
        assert_eq!(inner_copper_index("IN10.CU"), Some(10));
    }
}
