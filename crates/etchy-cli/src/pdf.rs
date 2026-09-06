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

/// Whole-run rasterized-pixel budget across BOTH documents. `diff_pdfs` holds
/// every page of both PDFs as RGBA at once, so the per-page cap alone lets
/// pages × pixels grow without bound (two 1000-page A4 documents at 150 DPI are
/// ~8.7 GB each — the process was OOM-killed with no error and no exit 2, #297).
/// 400 MP × 4 bytes/px ≈ 1.6 GB of page rasters resident (plus an overlay per
/// paired page); at 150 DPI an A4 sheet is ~2.2 MP, so ~90 sheets per side.
pub const MAX_TOTAL_PIXELS: u64 = 400_000_000;

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
    /// 1-based row number in the merged document (its reading order). With an
    /// inserted or removed sheet this is neither side's page number — those are
    /// `old_page` / `new_page`.
    pub page: usize,
    /// 1-based page number in the old PDF; absent for an inserted sheet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_page: Option<usize>,
    /// 1-based page number in the new PDF; absent for a removed sheet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_page: Option<usize>,
    pub present: Presence,
    pub added_px: u64,
    pub removed_px: u64,
    pub changed_px: u64,
    pub total_px: u64,
    pub changed_fraction: f64,
    pub regions: u32,
    /// Pixels in changed regions hidden by the `--min-region-px` noise floor.
    /// `0` at the default floor. Reported so a hidden change is never invisible.
    pub suppressed_px: u64,
    /// Number of sub-floor changed regions the noise floor hid.
    pub suppressed_regions: u32,
    /// Present when this sheet's paper size changed between revisions (#262):
    /// the two rasters have different dimensions, so there is no pixel diff and
    /// the whole page counts as changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_change: Option<PdfSizeChange>,
    /// Present when the two rasters differed only by rasterization rounding and
    /// were cropped to their shared region before diffing (#262). The page still
    /// has a real pixel diff and a real overlay — this just says which pixels
    /// were compared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounding_crop: Option<PdfRoundingCrop>,
    /// Filename of the overlay PNG written under `--out`, when given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overlay_png: Option<String>,
}

impl PdfPageReport {
    /// Includes sub-floor (`suppressed_*`) changes: a change hidden from the
    /// overlay by the noise floor is still a change (the trust bar). A resized
    /// sheet is a change too, even though it has no changed pixels to count.
    fn changed(&self) -> bool {
        self.present != Presence::Both
            || self.size_change.is_some()
            || self.changed_fraction > 0.0
            || self.suppressed_px > 0
    }
}

/// A sheet's paper-size change, in pixels at the diff's DPI (#262).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PdfSizeChange {
    pub old_width: u32,
    pub old_height: u32,
    pub new_width: u32,
    pub new_height: u32,
}

impl fmt::Display for PdfSizeChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} px -> {}x{} px",
            self.old_width, self.old_height, self.new_width, self.new_height
        )
    }
}

/// A sheet's rounding crop, in pixels at the diff's DPI (#262).
#[derive(Clone, Copy, Debug, Serialize)]
pub struct PdfRoundingCrop {
    pub old_width: u32,
    pub old_height: u32,
    pub new_width: u32,
    pub new_height: u32,
    pub diffed_width: u32,
    pub diffed_height: u32,
}

impl fmt::Display for PdfRoundingCrop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} px / {}x{} px, diffed over the shared {}x{} px",
            self.old_width,
            self.old_height,
            self.new_width,
            self.new_height,
            self.diffed_width,
            self.diffed_height
        )
    }
}

/// Which rule produced the page pairing (#249).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PdfPairingBasis {
    /// Plain page-index pairing — nothing was re-paired.
    Index,
    /// Content alignment, adopted because it clearly beat index pairing.
    Content,
    /// Content alignment was declined for want of clear evidence; index pairing
    /// was kept. Reported so a declined re-pairing is never silent.
    Ambiguous,
}

/// How the two revisions' pages were paired (#249). Always present in the JSON
/// so a consumer can see the pairing that produced the per-page rows; `identity`
/// is the plain index pairing with nothing inserted or removed.
#[derive(Serialize)]
pub struct PdfAlignmentReport {
    pub identity: bool,
    /// Which rule chose the pairing.
    pub basis: PdfPairingBasis,
    /// 1-based new-side page numbers of inserted sheets.
    pub inserted_new_pages: Vec<usize>,
    /// 1-based old-side page numbers of removed sheets.
    pub removed_old_pages: Vec<usize>,
    /// How many sheets paired — i.e. got a full pixel diff.
    pub paired: usize,
    /// The one-line statement of the alignment; absent for the identity, where
    /// there is nothing to explain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The whole-document PDF diff report (JSON schema v1 for PDF inputs).
#[derive(Serialize)]
pub struct PdfReport {
    pub schema_version: u32,
    pub tool_version: &'static str,
    pub any_changes: bool,
    pub dpi: f32,
    /// The active noise floor: changed regions smaller than this many pixels are
    /// hidden from the overlay/tallies (but still reported as `suppressed_*` and
    /// still count as a change). Default 1 = nothing hidden.
    pub min_region_px: u32,
    pub old_pages: usize,
    pub new_pages: usize,
    pub alignment: PdfAlignmentReport,
    pub pages: Vec<PdfPageReport>,
}

impl PdfReport {
    /// Build the report from an engine diff. The engine hands back one row per
    /// sheet of the merged document in reading order — paired sheets carry their
    /// pixel stats, inserted / removed sheets are explicit `new-only` /
    /// `old-only` rows sitting where they actually occur (#249).
    pub fn from_diff(diff: &PdfDiff, dpi: f32, min_region_px: u32) -> Self {
        let pages: Vec<PdfPageReport> = diff
            .pages
            .iter()
            .map(|p| {
                let present = match (p.old_page, p.new_page) {
                    (Some(_), Some(_)) => Presence::Both,
                    (Some(_), None) => Presence::OldOnly,
                    _ => Presence::NewOnly,
                };
                let s = p.diff.as_ref().map(|d| d.stats);
                PdfPageReport {
                    page: p.page,
                    old_page: p.old_page,
                    new_page: p.new_page,
                    present,
                    added_px: s.map_or(0, |s| s.added_px),
                    removed_px: s.map_or(0, |s| s.removed_px),
                    changed_px: s.map_or(0, |s| s.changed_px),
                    total_px: s.map_or(0, |s| s.total_px),
                    // An undiffed sheet is wholly a change: it appeared or
                    // disappeared, so the whole page is the difference.
                    changed_fraction: s.map_or(1.0, |s| s.changed_fraction),
                    regions: s.map_or(0, |s| s.regions),
                    suppressed_px: s.map_or(0, |s| s.suppressed_px),
                    suppressed_regions: s.map_or(0, |s| s.suppressed_regions),
                    size_change: p.size_change.map(|c| PdfSizeChange {
                        old_width: c.old.0,
                        old_height: c.old.1,
                        new_width: c.new.0,
                        new_height: c.new.1,
                    }),
                    rounding_crop: p.rounding_crop.map(|c| PdfRoundingCrop {
                        old_width: c.old.0,
                        old_height: c.old.1,
                        new_width: c.new.0,
                        new_height: c.new.1,
                        diffed_width: c.to.0,
                        diffed_height: c.to.1,
                    }),
                    overlay_png: None,
                }
            })
            .collect();
        let al = &diff.alignment;
        PdfReport {
            schema_version: PDF_SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION"),
            any_changes: diff.any_changes(),
            dpi,
            min_region_px,
            old_pages: diff.old_pages,
            new_pages: diff.new_pages,
            alignment: PdfAlignmentReport {
                identity: al.is_identity(),
                basis: match al.pairing {
                    etchy_core::Pairing::Index => PdfPairingBasis::Index,
                    etchy_core::Pairing::Content => PdfPairingBasis::Content,
                    etchy_core::Pairing::Ambiguous => PdfPairingBasis::Ambiguous,
                },
                inserted_new_pages: al.inserted_pages(),
                removed_old_pages: al.removed_pages(),
                paired: al.paired_count(),
                note: al.note(),
            },
            pages,
        }
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("PdfReport serializes")
    }

    /// The `--format summary` terminal table. The `old`/`new` columns spell out
    /// which sheet of each revision the row compares, so a re-pairing is legible
    /// rather than implied by the row number.
    pub fn to_summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{:<6} {:>4} {:>4} {:<9} {:>10} {:>10} {:>10} {:>9} {:>8}\n",
            "page",
            "old",
            "new",
            "present",
            "added_px",
            "removed_px",
            "changed_px",
            "changed%",
            "regions"
        ));
        for p in &self.pages {
            out.push_str(&format!(
                "{:<6} {:>4} {:>4} {:<9} {:>10} {:>10} {:>10} {:>8.3}% {:>8}\n",
                p.page,
                side(p.old_page),
                side(p.new_page),
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
        if let Some(note) = &self.alignment.note {
            out.push_str(note);
            out.push('\n');
        }
        for line in self.size_change_lines() {
            out.push_str(&line);
            out.push('\n');
        }
        for line in self.rounding_crop_lines() {
            out.push_str(&line);
            out.push('\n');
        }
        if let Some(note) = self.noise_floor_line() {
            out.push_str(&note);
            out.push('\n');
        }
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
        out.push('\n');
        if let Some(note) = &self.alignment.note {
            out.push('\n');
            out.push_str(note);
            out.push('\n');
        }
        out.push('\n');
        out.push_str(
            "| page | old | new | present | added px | removed px | changed px | changed % \
             | regions |\n",
        );
        out.push_str("|---:|---:|---:|---|---:|---:|---:|---:|---:|\n");
        for p in &self.pages {
            // Bold the page number of changed rows so they stand out in a
            // PR-comment render without any decoration.
            let page = if p.changed() {
                format!("**{}**", p.page)
            } else {
                p.page.to_string()
            };
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {:.3}% | {} |\n",
                page,
                side(p.old_page),
                side(p.new_page),
                p.present,
                p.added_px,
                p.removed_px,
                p.changed_px,
                p.changed_fraction * 100.0,
                p.regions
            ));
        }
        for line in self
            .size_change_lines()
            .iter()
            .chain(&self.rounding_crop_lines())
        {
            out.push('\n');
            out.push_str(line);
            out.push('\n');
        }
        if let Some(note) = self.noise_floor_line() {
            out.push('\n');
            out.push_str(&note);
            out.push('\n');
        }
        out.push('\n');
        out.push_str(if self.any_changes {
            "**Result: differences found.**"
        } else {
            "**Result: no differences.**"
        });
        out
    }

    /// One line per resized sheet, naming both sizes. A resized page reports
    /// 100% changed with zero changed pixels, which reads as a tool bug unless
    /// the reason is spelled out — so this is never optional when it applies.
    fn size_change_lines(&self) -> Vec<String> {
        self.pages
            .iter()
            .filter_map(|p| {
                let c = p.size_change?;
                Some(format!(
                    "size change: page {} ({c}) — the sheet's page size changed, so the whole \
                     page counts as changed and it has no pixel overlay",
                    p.page
                ))
            })
            .collect()
    }

    /// One line per sheet whose two rasters differed only by rasterization
    /// rounding (#262). The page IS diffed — this says which pixels were compared,
    /// so the handful of cropped edge pixels is never an unexplained gap.
    fn rounding_crop_lines(&self) -> Vec<String> {
        self.pages
            .iter()
            .filter_map(|p| {
                let c = p.rounding_crop?;
                Some(format!(
                    "page size rounding: page {} ({c}) — the two renders round to \
                     within {} px per axis, so it is the same sheet size and was \
                     diffed over the pixels they share",
                    p.page,
                    etchy_core::SIZE_TOLERANCE_PX
                ))
            })
            .collect()
    }

    /// A note about the noise floor, when it is armed above the default or has
    /// actually hidden something. Kept out of the "no differences" happy path so
    /// the default (floor 1, nothing hidden) stays quiet — but any suppression is
    /// always surfaced so a hidden change can never look like no change.
    fn noise_floor_line(&self) -> Option<String> {
        let hidden_px: u64 = self.pages.iter().map(|p| p.suppressed_px).sum();
        let hidden_regions: u32 = self.pages.iter().map(|p| p.suppressed_regions).sum();
        if hidden_regions == 0 && self.min_region_px <= 1 {
            return None;
        }
        Some(format!(
            "noise floor: --min-region-px {} hid {} region(s) / {} px \
             (still counted as changes)",
            self.min_region_px, hidden_regions, hidden_px
        ))
    }

    /// The page-count sentence — prominent in every format so an added or removed
    /// sheet, or a re-pairing, can never read as "no change".
    fn page_count_line(&self) -> String {
        let changed = self.pages.iter().filter(|p| p.changed()).count();
        let resized = self
            .pages
            .iter()
            .filter(|p| p.size_change.is_some())
            .count();
        if self.old_pages == self.new_pages && self.alignment.identity {
            if resized > 0 {
                // "N page(s) diffed" would be a lie: a resized sheet has no pixel
                // diff. Say how many were actually compared pixel-for-pixel.
                return format!(
                    "{} page(s) at {} DPI; {} pixel-diffed, {} resized, {} changed",
                    self.old_pages,
                    self.dpi,
                    self.old_pages - resized,
                    resized,
                    changed
                );
            }
            format!(
                "{} page(s) diffed at {} DPI; {} changed",
                self.old_pages, self.dpi, changed
            )
        } else {
            format!(
                "old {} page(s), new {} — {} sheet(s) paired at {} DPI; {} changed",
                self.old_pages, self.new_pages, self.alignment.paired, self.dpi, changed
            )
        }
    }
}

/// A page-number cell: the 1-based number, or `-` for the side that has no such
/// sheet. Never blank — an empty cell reads as a missing value, not as absence.
fn side(page: Option<usize>) -> String {
    page.map_or_else(|| "-".to_string(), |p| p.to_string())
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

    // Enforce the page-count, per-page and whole-run pixel ceilings BEFORE
    // rasterizing anything.
    let mut doc_px = [0u64; 2];
    let mut page_count = [0usize; 2];
    for (n, (label, path, bytes)) in [("old", &cli.old, &old), ("new", &cli.new, &new)]
        .into_iter()
        .enumerate()
    {
        let dims = etchy_pdf::page_pixel_dims(bytes, dpi)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("reading {label} PDF {}", path.display()))?;
        // Page alignment is a DP matrix quadratic in the page count, so a huge
        // document pair would ask for gigabytes. Fail loud here, naming the input
        // and the limit, before a single page is rendered (#249).
        page_count[n] = dims.len();
        if dims.len() > etchy_core::MAX_ALIGN_PAGES {
            anyhow::bail!(
                "{label} PDF {} has {} pages, over the {}-page limit etchy will align \
                 (page alignment cost grows with the square of the page count) — split \
                 the document",
                path.display(),
                dims.len(),
                etchy_core::MAX_ALIGN_PAGES
            );
        }
        for (i, (w, h)) in dims.iter().enumerate() {
            let px = u64::from(*w) * u64::from(*h);
            // A page that floors to zero pixels would "diff" nothing at all and
            // read as no-change — a silent false negative. Fail loud instead.
            if px == 0 {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}×{h} px at {dpi} DPI — \
                     nothing to compare; raise --dpi",
                    i + 1
                );
            }
            if px > MAX_PAGE_PIXELS {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}×{h} px (~{} MP) at {dpi} DPI, \
                     over the ~{} MP per-page cap — lower --dpi",
                    i + 1,
                    px / 1_000_000,
                    MAX_PAGE_PIXELS / 1_000_000
                );
            }
            doc_px[n] += px;
        }
    }
    // Every page of both documents is resident as RGBA during the diff, so the
    // sum over pages × pixels has to be bounded too — under the per-page cap,
    // a many-page pair still OOM-kills the process with no error (#297).
    let total_px = doc_px[0] + doc_px[1];
    if total_px > MAX_TOTAL_PIXELS {
        anyhow::bail!(
            "this PDF pair would rasterize to ~{} MP in total (old {} pages ~{} MP + new {} \
             pages ~{} MP) at {dpi} DPI, over the ~{} MP total raster budget — lower --dpi",
            total_px / 1_000_000,
            page_count[0],
            doc_px[0] / 1_000_000,
            page_count[1],
            doc_px[1] / 1_000_000,
            MAX_TOTAL_PIXELS / 1_000_000
        );
    }

    let mut opts = etchy_core::ImageDiffOptions::default();
    if let Some(m) = cli.min_region_px {
        opts.min_region_px = m;
    }
    let diff = etchy_pdf::diff_pdfs(&old, &new, dpi, &opts)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("diffing PDFs")?;
    let mut report = PdfReport::from_diff(&diff, dpi, opts.min_region_px);

    // Optional per-page overlay PNGs (the viewable deliverable).
    if let Some(dir) = &cli.out {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating output directory {}", dir.display()))?;
        for p in &diff.pages {
            let Some(d) = &p.diff else {
                // No pixel diff means no overlay: the sheet exists on one side
                // only, or its page size changed. Say which on stderr — the row
                // is in the report either way, but a silently absent PNG invites
                // "did it miss it?".
                let why = match (p.size_change, p.old_page.is_some()) {
                    (Some(c), _) => format!("its page size changed ({c})"),
                    (None, true) => "it exists in the old revision only".to_string(),
                    (None, false) => "it exists in the new revision only".to_string(),
                };
                eprintln!("etchy: no overlay for page {} — {why}", p.page);
                continue;
            };
            let name = format!("page-{}.png", p.page);
            let png = etchy_pdf::encode_png(&d.overlay).map_err(|e| anyhow::anyhow!("{e}"))?;
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
    let out = match format {
        Format::Json => report.to_json_pretty(),
        Format::Md => report.to_markdown(),
        Format::Summary => report.to_summary(),
    };
    // Route through the shared writer so a closed pipe (`… | head`) exits cleanly
    // instead of panicking (#261).
    crate::write_stdout(&out)?;
    Ok(report.any_changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use etchy_core::{Image, ImageDiffResult, ImageDiffStats, PageAlignment, PageMatch, Pairing};
    use etchy_pdf::PageDiff;

    /// A paired row: sheet `page` on both sides, with `changed_px` changed pixels.
    fn page(page: usize, changed_px: u64, total_px: u64) -> PageDiff {
        page_with_suppressed(page, changed_px, total_px, 0, 0)
    }

    fn page_with_suppressed(
        page: usize,
        changed_px: u64,
        total_px: u64,
        suppressed_px: u64,
        suppressed_regions: u32,
    ) -> PageDiff {
        let changed_fraction = changed_px as f64 / total_px as f64;
        PageDiff {
            page,
            old_page: Some(page),
            new_page: Some(page),
            diff: Some(ImageDiffResult {
                stats: ImageDiffStats {
                    width: 10,
                    height: 10,
                    added_px: changed_px,
                    removed_px: 0,
                    changed_px,
                    total_px,
                    changed_fraction,
                    regions: u32::from(changed_px > 0),
                    suppressed_px,
                    suppressed_regions,
                },
                overlay: Image::new(1, 1, vec![0, 0, 0, 0]).unwrap(),
            }),
            size_change: None,
            rounding_crop: None,
        }
    }

    /// An unpaired row: a sheet present in one revision only.
    fn unpaired(page: usize, old_page: Option<usize>, new_page: Option<usize>) -> PageDiff {
        PageDiff {
            page,
            old_page,
            new_page,
            diff: None,
            size_change: None,
            rounding_crop: None,
        }
    }

    /// A diff whose pages paired 1:1 by index (the common case).
    fn diff(old_pages: usize, new_pages: usize, pages: Vec<PageDiff>) -> PdfDiff {
        PdfDiff {
            old_pages,
            new_pages,
            pages,
            alignment: PageAlignment::by_index(old_pages, new_pages),
        }
    }

    /// A diff with an explicit alignment (for the re-pairing cases).
    fn aligned_diff(
        old_pages: usize,
        new_pages: usize,
        matches: Vec<PageMatch>,
        pages: Vec<PageDiff>,
    ) -> PdfDiff {
        PdfDiff {
            old_pages,
            new_pages,
            pages,
            alignment: PageAlignment {
                matches,
                pairing: Pairing::Content,
            },
        }
    }

    #[test]
    fn identical_pdfs_report_no_changes() {
        let r = PdfReport::from_diff(
            &diff(2, 2, vec![page(1, 0, 100), page(2, 0, 100)]),
            150.0,
            1,
        );
        assert!(!r.any_changes);
        assert_eq!(r.schema_version, PDF_SCHEMA_VERSION);
        assert_eq!(r.pages.len(), 2);
        assert!(r.pages.iter().all(|p| p.present == Presence::Both));
        assert!(r.to_summary().contains("result: no differences"));
        assert!(r.to_markdown().contains("no differences"));
    }

    #[test]
    fn a_changed_page_reports_its_pixels() {
        let r = PdfReport::from_diff(&diff(1, 1, vec![page(1, 30, 100)]), 150.0, 1);
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
        let r = PdfReport::from_diff(
            &diff(
                1,
                3,
                vec![
                    page(1, 0, 100),
                    unpaired(2, None, Some(2)),
                    unpaired(3, None, Some(3)),
                ],
            ),
            150.0,
            1,
        );
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
        let summary = r.to_summary();
        assert!(
            summary.contains("old 1 page(s), new 3"),
            "counts are spelled out: {summary}"
        );
        assert!(
            summary.contains("1 sheet(s) paired"),
            "how many sheets were actually diffed: {summary}"
        );
        assert!(
            summary.contains("2 sheets inserted at new pages 2, 3"),
            "the alignment names the inserted sheets: {summary}"
        );
    }

    #[test]
    fn a_removed_page_is_old_only() {
        let r = PdfReport::from_diff(
            &diff(2, 1, vec![page(1, 0, 100), unpaired(2, Some(2), None)]),
            150.0,
            1,
        );
        assert!(r.any_changes, "a page disappearing is a change");
        assert_eq!(r.pages[1].present, Presence::OldOnly);
        assert!(r.to_summary().contains("old-only"));
        assert!(r.to_json_pretty().contains("\"old-only\""));
        assert!(r.to_summary().contains("1 sheet removed at old page 2"));
    }

    #[test]
    fn an_inserted_sheet_sits_mid_document_and_the_alignment_is_reported() {
        // #249: old [A, B, C]; new [A, X, B, C]. The inserted sheet is row 2 and
        // the later sheets pair across a page-number shift — which every format
        // must state, since which sheets were compared is not the row number.
        let r = PdfReport::from_diff(
            &aligned_diff(
                3,
                4,
                vec![
                    PageMatch::Paired { old: 0, new: 0 },
                    PageMatch::NewOnly { new: 1 },
                    PageMatch::Paired { old: 1, new: 2 },
                    PageMatch::Paired { old: 2, new: 3 },
                ],
                vec![
                    page(1, 0, 100),
                    unpaired(2, None, Some(2)),
                    PageDiff {
                        page: 3,
                        old_page: Some(2),
                        new_page: Some(3),
                        ..page(3, 0, 100)
                    },
                    PageDiff {
                        page: 4,
                        old_page: Some(3),
                        new_page: Some(4),
                        ..page(4, 0, 100)
                    },
                ],
            ),
            150.0,
            1,
        );
        assert!(r.any_changes, "an inserted sheet is a change");
        assert_eq!(r.pages[1].present, Presence::NewOnly);
        assert_eq!((r.pages[1].old_page, r.pages[1].new_page), (None, Some(2)));
        assert_eq!(
            (r.pages[2].old_page, r.pages[2].new_page),
            (Some(2), Some(3))
        );
        // Every format states the alignment chosen — never silently re-paired.
        for text in [r.to_summary(), r.to_markdown()] {
            assert!(
                text.contains("aligned by page content")
                    && text.contains("1 sheet inserted at new page 2"),
                "the alignment must be reported: {text}"
            );
        }
        let json = r.to_json_pretty();
        assert!(json.contains("\"identity\": false"), "{json}");
        assert!(
            json.contains("\"inserted_new_pages\": [\n      2\n    ]"),
            "{json}"
        );
        assert!(json.contains("\"paired\": 3"), "{json}");
        assert!(json.contains("\"old_page\": 2"), "{json}");
        // Only the changed rows are marked changed: the re-paired sheets are clean.
        assert_eq!(r.pages.iter().filter(|p| p.changed()).count(), 1);
    }

    #[test]
    fn a_page_size_change_is_a_fully_changed_page_with_the_sizes_named() {
        // #262: a resized sheet is a diff, not an error. The row is wholly
        // changed and every format says WHY — otherwise a 100%-changed page with
        // zero changed pixels looks like a bug in the tool.
        let r = PdfReport::from_diff(
            &diff(
                1,
                1,
                vec![PageDiff {
                    page: 1,
                    old_page: Some(1),
                    new_page: Some(1),
                    diff: None,
                    size_change: Some(etchy_pdf::SizeChange {
                        old: (1240, 1754),
                        new: (1754, 2480),
                    }),
                    rounding_crop: None,
                }],
            ),
            150.0,
            1,
        );
        assert!(r.any_changes, "a resized sheet is a change");
        let p = &r.pages[0];
        assert_eq!(
            p.present,
            Presence::Both,
            "the sheet is still on both sides"
        );
        assert_eq!(p.changed_fraction, 1.0, "the whole sheet counts as changed");
        assert!(p.changed());
        for text in [r.to_summary(), r.to_markdown()] {
            assert!(
                text.contains("size change") && text.contains("1240x1754"),
                "the size change and both sizes are named: {text}"
            );
        }
        let json = r.to_json_pretty();
        assert!(json.contains("\"old_width\": 1240"), "{json}");
        assert!(json.contains("\"new_height\": 2480"), "{json}");
    }

    #[test]
    fn a_rounding_crop_is_reported_and_the_page_still_has_its_diff() {
        // #262 tolerance: a sheet whose two renders round a pixel apart is the SAME
        // size, so it keeps its pixel diff — and every format says which pixels
        // were compared, so the cropped edge is never an unexplained gap.
        let r = PdfReport::from_diff(
            &diff(
                1,
                1,
                vec![PageDiff {
                    rounding_crop: Some(etchy_pdf::RoundingCrop {
                        old: (1754, 1239),
                        new: (1753, 1240),
                        to: (1753, 1239),
                    }),
                    ..page(1, 30, 100)
                }],
            ),
            150.0,
            1,
        );
        let p = &r.pages[0];
        assert_eq!(p.present, Presence::Both);
        assert!(p.size_change.is_none(), "rounding is not a size change");
        assert_eq!(p.changed_px, 30, "the page still carries its pixel diff");
        assert!(r.any_changes);
        for text in [r.to_summary(), r.to_markdown()] {
            assert!(
                text.contains("page size rounding")
                    && text.contains("1754x1239")
                    && text.contains("1753x1239"),
                "the crop is named in every format: {text}"
            );
        }
        let json = r.to_json_pretty();
        assert!(json.contains("\"diffed_width\": 1753"), "{json}");
        assert!(json.contains("\"new_height\": 1240"), "{json}");
        // An uncropped page says nothing about cropping.
        let clean = PdfReport::from_diff(&diff(1, 1, vec![page(1, 3, 100)]), 150.0, 1);
        assert!(!clean.to_summary().contains("page size rounding"));
        assert!(!clean.to_json_pretty().contains("rounding_crop"));
    }

    #[test]
    fn a_declined_realignment_is_reported_as_ambiguous() {
        // The confidence net (#249): when content alignment is not clearly better
        // than index pairing it is declined — and that has to be visible, or a
        // silent fallback looks like the alignment simply found nothing.
        let r = PdfReport::from_diff(
            &PdfDiff {
                old_pages: 2,
                new_pages: 2,
                pages: vec![page(1, 0, 100), page(2, 5, 100)],
                alignment: PageAlignment {
                    matches: PageAlignment::by_index(2, 2).matches,
                    pairing: Pairing::Ambiguous,
                },
            },
            150.0,
            1,
        );
        assert_eq!(r.alignment.basis, PdfPairingBasis::Ambiguous);
        for text in [r.to_summary(), r.to_markdown()] {
            assert!(
                text.contains("page alignment was ambiguous") && text.contains("index"),
                "the fallback is stated: {text}"
            );
        }
        assert!(
            r.to_json_pretty().contains("\"basis\": \"ambiguous\""),
            "{}",
            r.to_json_pretty()
        );
    }

    #[test]
    fn an_identity_alignment_says_nothing_extra() {
        // The quiet path: same page count, index pairing — the report reads
        // exactly as it always did, with no alignment commentary.
        let r = PdfReport::from_diff(
            &diff(2, 2, vec![page(1, 0, 100), page(2, 4, 100)]),
            150.0,
            1,
        );
        assert!(r.alignment.identity);
        assert_eq!(r.alignment.basis, PdfPairingBasis::Index);
        assert_eq!(r.alignment.note, None);
        for text in [r.to_summary(), r.to_markdown()] {
            assert!(!text.contains("aligned by page content"), "{text}");
        }
        assert!(r
            .to_summary()
            .contains("2 page(s) diffed at 150 DPI; 1 changed"));
    }

    #[test]
    fn json_carries_the_schema_and_overlay_name() {
        let mut r = PdfReport::from_diff(&diff(1, 1, vec![page(1, 5, 100)]), 300.0, 1);
        r.pages[0].overlay_png = Some("page-1.png".into());
        let json = r.to_json_pretty();
        assert!(json.contains("\"schema_version\": 1"));
        assert!(json.contains("\"dpi\": 300.0"));
        assert!(json.contains("\"overlay_png\": \"page-1.png\""));
        // Unwritten overlays are omitted, not null.
        let r2 = PdfReport::from_diff(&diff(1, 1, vec![page(1, 5, 100)]), 300.0, 1);
        assert!(!r2.to_json_pretty().contains("overlay_png"));
    }

    #[test]
    fn a_sub_floor_change_is_surfaced_and_counts_as_a_difference() {
        // #260: a page whose only change is below the noise floor has zero
        // above-floor pixels (changed_fraction 0.0) but MUST still read as changed
        // and must surface the hidden region — never a silent "no differences".
        let r = PdfReport::from_diff(
            &diff(1, 1, vec![page_with_suppressed(1, 0, 100, 1, 1)]),
            150.0,
            2,
        );
        assert!(r.any_changes, "a hidden sub-floor change is still a change");
        assert!(r.pages[0].changed(), "the page is marked changed");
        // Every format surfaces the suppression, not just the JSON.
        let summary = r.to_summary();
        assert!(summary.contains("result: differences found"));
        assert!(
            summary.contains("noise floor") && summary.contains("min-region-px"),
            "summary surfaces the noise floor: {summary}"
        );
        assert!(r.to_markdown().contains("noise floor"));
        let json = r.to_json_pretty();
        assert!(json.contains("\"suppressed_px\": 1"));
        assert!(json.contains("\"suppressed_regions\": 1"));
        assert!(json.contains("\"min_region_px\": 2"));
    }

    #[test]
    fn the_default_floor_stays_quiet_when_nothing_is_hidden() {
        // At the default floor (1, hides nothing) with no suppression, no noise-floor
        // note clutters the happy path.
        let r = PdfReport::from_diff(&diff(1, 1, vec![page(1, 0, 100)]), 150.0, 1);
        assert!(!r.any_changes);
        assert!(!r.to_summary().contains("noise floor"));
        assert!(!r.to_markdown().contains("noise floor"));
    }
}
