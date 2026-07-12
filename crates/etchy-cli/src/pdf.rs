//! Schematic-PDF diff CLI path (CLI-6). Compiled only with the `pdf` feature.
//!
//! PDF has no layers or mm², so it cannot reuse the Gerber `DiffReport`; this is
//! the parallel, separately schema-versioned report over the pixel diff that
//! `etchy_pdf::diff_pdfs` produces. Trust rules carried over from the geometry
//! path: pages that exist on only one side are listed **explicitly** in every
//! output format (a sheet appearing or disappearing is a change, never a silent
//! skip), and oversized inputs fail loud before allocating.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use etchy_pdf::PdfDiff;
use serde::Serialize;

use crate::{Cli, Format, MAX_LAYER_FILE_BYTES};

/// PDF report schema, versioned independently of the Gerber JSON v1.
pub const PDF_SCHEMA_VERSION: u32 = 1;

/// Per-page rasterized-pixel ceiling (~50 megapixels ≈ 200 MB of RGBA). A large
/// sheet at high `--dpi` is a DoS-sized allocation; over the cap we fail loud
/// *before* rendering, consistent with the Gerber per-file caps (CORE-7).
pub const MAX_PAGE_PIXELS: u64 = 50_000_000;

/// Whether a page exists in both revisions or only one — page-count changes must
/// be legible in every format, not folded into a count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Presence {
    Both,
    OldOnly,
    NewOnly,
}

impl fmt::Display for Presence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Presence::Both => "both",
            Presence::OldOnly => "old-only",
            Presence::NewOnly => "new-only",
        })
    }
}

/// One page of the PDF diff. For `old-only` / `new-only` pages the pixel counts
/// are zero (the engine does not diff unpaired pages) but `changed_fraction` is
/// 1.0 — the whole sheet appeared or disappeared, which is a change.
#[derive(Serialize)]
pub struct PdfPageReport {
    /// 1-based page number.
    pub page: usize,
    pub present: Presence,
    pub added_px: u64,
    pub removed_px: u64,
    pub changed_px: u64,
    pub total_px: u64,
    pub changed_fraction: f64,
    pub regions: u32,
    /// Filename of the overlay PNG written under `--out`, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_png: Option<String>,
}

impl PdfPageReport {
    fn changed(&self) -> bool {
        self.present != Presence::Both || self.changed_fraction > 0.0
    }
}

/// The whole-document PDF diff report (JSON schema v1 for PDF inputs).
#[derive(Serialize)]
pub struct PdfReport {
    pub schema_version: u32,
    pub tool_version: &'static str,
    pub any_changes: bool,
    pub dpi: f32,
    pub old_pages: usize,
    pub new_pages: usize,
    pub pages: Vec<PdfPageReport>,
}

impl PdfReport {
    /// Build the report from an engine diff: paired pages carry their pixel
    /// stats; extra pages on either side are appended as explicit
    /// `old-only` / `new-only` rows with 1-based numbering.
    pub fn from_diff(diff: &PdfDiff, dpi: f32) -> Self {
        let mut pages: Vec<PdfPageReport> = diff
            .pages
            .iter()
            .map(|p| PdfPageReport {
                page: p.page,
                present: Presence::Both,
                added_px: p.diff.stats.added_px,
                removed_px: p.diff.stats.removed_px,
                changed_px: p.diff.stats.changed_px,
                total_px: p.diff.stats.total_px,
                changed_fraction: p.diff.stats.changed_fraction,
                regions: p.diff.stats.regions,
                overlay_png: None,
            })
            .collect();
        let paired = diff.old_pages.min(diff.new_pages);
        let (extra, presence) = if diff.old_pages > diff.new_pages {
            (diff.old_pages, Presence::OldOnly)
        } else {
            (diff.new_pages, Presence::NewOnly)
        };
        for page in (paired + 1)..=extra {
            pages.push(PdfPageReport {
                page,
                present: presence,
                added_px: 0,
                removed_px: 0,
                changed_px: 0,
                total_px: 0,
                changed_fraction: 1.0,
                regions: 0,
                overlay_png: None,
            });
        }
        PdfReport {
            schema_version: PDF_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION"),
            any_changes: diff.any_changes(),
            dpi,
            old_pages: diff.old_pages,
            new_pages: diff.new_pages,
            pages,
        }
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("PdfReport serializes")
    }

    /// The `--format summary` terminal table.
    pub fn to_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{:<6} {:<9} {:>10} {:>10} {:>10} {:>9} {:>8}\n",
            "page", "present", "added_px", "removed_px", "changed_px", "changed%", "regions"
        ));
        for p in &self.pages {
            out.push_str(&format!(
                "{:<6} {:<9} {:>10} {:>10} {:>10} {:>8.3}% {:>8}\n",
                p.page,
                p.present,
                p.added_px,
                p.removed_px,
                p.changed_px,
                p.changed_fraction * 100.0,
                p.regions
            ));
        }
        out.push('\n');
        out.push_str(&self.page_count_line());
        out.push('\n');
        out.push_str(if self.any_changes {
            "result: differences found"
        } else {
            "result: no differences"
        });
        out
    }

    /// The `--format md` GitHub-Markdown table (CI step-summary / PR comment).
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("## etchy PDF diff\n\n");
        out.push_str(&self.page_count_line());
        out.push_str("\n\n");
        out.push_str(
            "| page | present | added px | removed px | changed px | changed % | regions |\n",
        );
        out.push_str("|---:|---|---:|---:|---:|---:|---:|\n");
        for p in &self.pages {
            // Bold the page number of changed rows so they stand out in a
            // PR-comment render without any decoration.
            let page = if p.changed() {
                format!("**{}**", p.page)
            } else {
                p.page.to_string()
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.3}% | {} |\n",
                page,
                p.present,
                p.added_px,
                p.removed_px,
                p.changed_px,
                p.changed_fraction * 100.0,
                p.regions
            ));
        }
        out.push('\n');
        out.push_str(if self.any_changes {
            "**Result: differences found.**"
        } else {
            "**Result: no differences.**"
        });
        out
    }

    /// The page-count sentence — prominent in every format so an added or removed
    /// sheet can never read as "no change".
    fn page_count_line(&self) -> String {
        let paired = self.old_pages.min(self.new_pages);
        if self.old_pages == self.new_pages {
            let changed = self.pages.iter().filter(|p| p.changed()).count();
            format!(
                "{} page(s) diffed at {} DPI; {} changed",
                paired, self.dpi, changed
            )
        } else {
            let (extra, side) = if self.old_pages > self.new_pages {
                (self.old_pages, "old-only (removed)")
            } else {
                (self.new_pages, "new-only (added)")
            };
            let list = ((paired + 1)..=extra)
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "old {} page(s), new {} — pages 1–{} diffed at {} DPI; page(s) {} {}",
                self.old_pages, self.new_pages, paired, self.dpi, list, side
            )
        }
    }
}

/// Read one PDF input, enforcing the shared per-file byte cap before it hits RAM.
fn read_pdf(path: &Path) -> Result<Vec<u8>> {
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
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// The PDF branch of `run()`: guards, diff, optional overlay PNGs, output.
/// Returns whether any change was found (the caller maps it to exit 0/1).
pub fn run_pdf(cli: &Cli) -> Result<bool> {
    // The copper gates are mm²/layer concepts — meaningless for pixels. Refuse
    // loudly rather than silently ignoring a CI gate the user thinks is armed.
    if cli.fail_on_area.is_some()
        || cli.fail_on_regions.is_some()
        || !matches!(
            cli.gate_layers.trim().to_ascii_lowercase().as_str(),
            "" | "all"
        )
    {
        anyhow::bail!(
            "--fail-on-area / --fail-on-regions / --gate-layers are not valid for PDF \
             inputs (pixel diffs have no layers or mm²); any change exits 1"
        );
    }
    // SVG/HTML are vector outputs of the geometry path; PDF overlays are raster.
    if cli.svg.is_some() || cli.html.is_some() {
        anyhow::bail!(
            "--svg / --html are not valid for PDF inputs; use --out DIR for per-page \
             overlay PNGs"
        );
    }

    let dpi = cli.dpi.unwrap_or(etchy_pdf::DEFAULT_DPI);
    if !dpi.is_finite() || dpi <= 0.0 {
        anyhow::bail!("--dpi must be a positive number, got {dpi}");
    }

    let old = read_pdf(&cli.old)?;
    let new = read_pdf(&cli.new)?;

    // Enforce the per-page pixel ceiling BEFORE rasterizing anything.
    for (label, path, bytes) in [("old", &cli.old, &old), ("new", &cli.new, &new)] {
        let dims = etchy_pdf::page_pixel_dims(bytes, dpi)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("reading {label} PDF {}", path.display()))?;
        for (i, (w, h)) in dims.iter().enumerate() {
            let px = u64::from(*w) * u64::from(*h);
            if px > MAX_PAGE_PIXELS {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}×{h} px (~{} MP) at {dpi} DPI, \
                     over the ~{} MP per-page cap — lower --dpi",
                    i + 1,
                    px / 1_000_000,
                    MAX_PAGE_PIXELS / 1_000_000
                );
            }
        }
    }

    let diff = etchy_pdf::diff_pdfs(&old, &new, dpi, &Default::default())
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("diffing PDFs")?;
    let mut report = PdfReport::from_diff(&diff, dpi);

    // Optional per-page overlay PNGs (the viewable deliverable).
    if let Some(dir) = &cli.out {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating output directory {}", dir.display()))?;
        for p in &diff.pages {
            let name = format!("page-{}.png", p.page);
            let png = etchy_pdf::encode_png(&p.diff.overlay).map_err(|e| anyhow::anyhow!("{e}"))?;
            let path = dir.join(&name);
            std::fs::write(&path, png).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("etchy: wrote {}", path.display());
            if let Some(row) = report.pages.iter_mut().find(|r| r.page == p.page) {
                row.overlay_png = Some(name);
            }
        }
    }

    // `--json` is the deprecated alias for `--format json`, same as the geometry path.
    let format = if cli.json { Format::Json } else { cli.format };
    match format {
        Format::Json => println!("{}", report.to_json_pretty()),
        Format::Md => println!("{}", report.to_markdown()),
        Format::Summary => println!("{}", report.to_summary()),
    }
    Ok(report.any_changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use etchy_core::{Image, ImageDiffResult, ImageDiffStats};
    use etchy_pdf::PageDiff;

    fn page(page: usize, changed_px: u64, total_px: u64) -> PageDiff {
        let changed_fraction = changed_px as f64 / total_px as f64;
        PageDiff {
            page,
            diff: ImageDiffResult {
                stats: ImageDiffStats {
                    width: 10,
                    height: 10,
                    added_px: changed_px,
                    removed_px: 0,
                    changed_px,
                    total_px,
                    changed_fraction,
                    regions: u32::from(changed_px > 0),
                },
                overlay: Image::new(1, 1, vec![0, 0, 0, 0]).unwrap(),
            },
        }
    }

    fn diff(old_pages: usize, new_pages: usize, pages: Vec<PageDiff>) -> PdfDiff {
        PdfDiff {
            old_pages,
            new_pages,
            pages,
        }
    }

    #[test]
    fn identical_pdfs_report_no_changes() {
        let r = PdfReport::from_diff(&diff(2, 2, vec![page(1, 0, 100), page(2, 0, 100)]), 150.0);
        assert!(!r.any_changes);
        assert_eq!(r.schema_version, PDF_SCHEMA_VERSION);
        assert_eq!(r.pages.len(), 2);
        assert!(r.pages.iter().all(|p| p.present == Presence::Both));
        assert!(r.to_summary().contains("result: no differences"));
        assert!(r.to_markdown().contains("no differences"));
    }

    #[test]
    fn a_changed_page_reports_its_pixels() {
        let r = PdfReport::from_diff(&diff(1, 1, vec![page(1, 30, 100)]), 150.0);
        assert!(r.any_changes);
        let p = &r.pages[0];
        assert_eq!((p.page, p.changed_px, p.total_px), (1, 30, 100));
        assert!((p.changed_fraction - 0.3).abs() < 1e-9);
        assert!(r.to_summary().contains("result: differences found"));
    }

    #[test]
    fn an_added_page_is_listed_explicitly_in_every_format() {
        // old has 1 page, new has 3 → pages 2 and 3 are new-only, and a page
        // appearing IS a change even when the paired page is identical.
        let r = PdfReport::from_diff(&diff(1, 3, vec![page(1, 0, 100)]), 150.0);
        assert!(r.any_changes, "a page appearing is a change");
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[1].present, Presence::NewOnly);
        assert_eq!(r.pages[2].page, 3, "1-based numbering continues");
        assert_eq!(r.pages[1].changed_fraction, 1.0);
        for text in [r.to_summary(), r.to_markdown(), r.to_json_pretty()] {
            assert!(
                text.contains("new-only"),
                "new-only must be legible: {text}"
            );
        }
        assert!(r.to_summary().contains("pages 1–1 diffed"));
        assert!(r.to_summary().contains("2, 3"));
    }

    #[test]
    fn a_removed_page_is_old_only() {
        let r = PdfReport::from_diff(&diff(2, 1, vec![page(1, 0, 100)]), 150.0);
        assert!(r.any_changes, "a page disappearing is a change");
        assert_eq!(r.pages[1].present, Presence::OldOnly);
        assert!(r.to_summary().contains("old-only"));
        assert!(r.to_json_pretty().contains("\"old-only\""));
    }

    #[test]
    fn json_carries_the_schema_and_overlay_name() {
        let mut r = PdfReport::from_diff(&diff(1, 1, vec![page(1, 5, 100)]), 300.0);
        r.pages[0].overlay_png = Some("page-1.png".into());
        let json = r.to_json_pretty();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"dpi\": 300.0"));
        assert!(json.contains("\"overlay_png\": \"page-1.png\""));
        // Unwritten overlays are omitted, not null.
        let r2 = PdfReport::from_diff(&diff(1, 1, vec![page(1, 5, 100)]), 300.0);
        assert!(!r2.to_json_pretty().contains("overlay_png"));
    }
}
