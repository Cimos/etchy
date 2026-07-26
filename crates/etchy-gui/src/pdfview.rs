//! PDF-mode view model for the GUI (#63): per-page rasters, their pixel diff, and
//! the pure page-row bookkeeping (pairing, presence, ordering) behind the Pages
//! panel. Mirrors the CLI's trust rules: a page that exists on only one side is an
//! explicit row, never a silent skip, and an oversized raster fails loud before
//! anything is allocated.
//!
//! The heavy lifting (hayro rasterization, `diff_images`) only compiles with the
//! `pdf` feature; the data model and the pure helpers are unconditional so the
//! rest of the GUI — and its tests — never need cfg forests. Without the feature
//! a [`PdfView`] simply cannot be constructed ([`build_pdf_view`] fails loud).

use eframe::egui;
use etchy_core::{Image, PageAlignment, PageMatch};

/// Whether the bytes are a PDF (`%PDF` magic at the start). Routes a loaded file
/// into PDF mode; mixing a PDF with Gerber input is a loud error upstream.
pub fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF")
}

/// Per-page rasterized-pixel ceiling (~50 megapixels ≈ 200 MB of RGBA), matching
/// the CLI's cap: a large sheet at high DPI is a DoS-sized allocation, so over the
/// cap we fail loud *before* rendering (consistent with the Gerber per-file caps).
#[cfg(feature = "pdf")]
pub const MAX_PAGE_PIXELS: u64 = 50_000_000;

/// Per-DIMENSION ceiling: egui/egui_glow assert a texture side fits the GPU's
/// `max_texture_side` and PANIC past it (native aborts, a wasm tab dies), and
/// the area cap alone would pass a long thin sheet. 8192 is safe on the desktop
/// GL / WebGL2 stacks etchy targets.
#[cfg(feature = "pdf")]
pub const MAX_TEXTURE_SIDE: u32 = 8192;

/// Whole-document raster budget across BOTH PDFs. Every paired page retains
/// old + new + overlay RGBA plus GPU textures (~4x total page bytes), so a
/// many-page pair must fail loud up front rather than OOM mid-load. 250 MP
/// ≈ 1 GB of page rasters (~55 A4 sheets per side at 150 DPI).
#[cfg(feature = "pdf")]
pub const MAX_DOC_PIXELS: u64 = 250_000_000;

/// Whether a page exists in both revisions or only one. A sheet appearing or
/// disappearing IS a change and must stay legible (trust: no silent misses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    Both,
    OldOnly,
    NewOnly,
}

impl Presence {
    /// The short row tag shown in the Pages panel; `None` for paired pages.
    pub fn tag(self) -> Option<&'static str> {
        match self {
            Presence::Both => None,
            Presence::OldOnly => Some("old only"),
            Presence::NewOnly => Some("new only"),
        }
    }
}

/// One row of the merged document: where the sheet sits in the reading order and
/// which page of each revision it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagePlanRow {
    /// 1-based row number in the merged document.
    pub row: usize,
    pub presence: Presence,
    /// 1-based page in the old PDF, when the sheet exists there.
    pub old_page: Option<usize>,
    /// 1-based page in the new PDF, when the sheet exists there.
    pub new_page: Option<usize>,
}

/// Turn a content alignment (`etchy_core::pagealign`, #249) into the Pages
/// panel's rows: paired sheets plus explicit inserted / removed ones, in reading
/// order. Pure → unit-testable, and available without the `pdf` feature.
pub fn page_plan(alignment: &PageAlignment) -> Vec<PagePlanRow> {
    alignment
        .matches
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let (presence, old_page, new_page) = match *m {
                PageMatch::Paired { old, new } => (Presence::Both, Some(old + 1), Some(new + 1)),
                PageMatch::OldOnly { old } => (Presence::OldOnly, Some(old + 1), None),
                PageMatch::NewOnly { new } => (Presence::NewOnly, None, Some(new + 1)),
            };
            PagePlanRow {
                row: i + 1,
                presence,
                old_page,
                new_page,
            }
        })
        .collect()
}

/// A paired sheet whose two sides rasterized to genuinely different pixel sizes —
/// its paper size changed between revisions (#262). Mirrors `etchy_pdf::SizeChange`,
/// kept local so the view model and its tests compile without the `pdf` feature.
/// A difference of a pixel or two per axis is rasterization rounding, not this —
/// see [`PageRoundingCrop`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSizeChange {
    pub old: (u32, u32),
    pub new: (u32, u32),
}

impl std::fmt::Display for PageSizeChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{} px -> {}x{} px",
            self.old.0, self.old.1, self.new.0, self.new.1
        )
    }
}

/// A paired sheet whose two rasters differed only by rasterization rounding
/// (within `etchy_core::SIZE_TOLERANCE_PX` per axis): the same sheet size, diffed
/// over the pixels the two renders share (#262). Mirrors
/// `etchy_pdf::RoundingCrop`, kept local for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRoundingCrop {
    pub old: (u32, u32),
    pub new: (u32, u32),
    /// The shared region both sides were cropped to before diffing.
    pub to: (u32, u32),
}

impl std::fmt::Display for PageRoundingCrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}x{} px / {}x{} px, diffed over the shared {}x{} px",
            self.old.0, self.old.1, self.new.0, self.new.1, self.to.0, self.to.1
        )
    }
}

/// A page row's label. Page numbers name the revision they belong to, so a sheet
/// that moved between revisions says so instead of hiding behind a row number
/// (#249: which sheets were compared must never be guesswork).
pub fn row_label(old_page: Option<usize>, new_page: Option<usize>) -> String {
    match (old_page, new_page) {
        (Some(o), Some(n)) if o == n => format!("page {o}"),
        (Some(o), Some(n)) => format!("page {n} (was {o})"),
        (Some(o), None) => format!("page {o}"),
        (None, Some(n)) => format!("page {n}"),
        (None, None) => "page ?".to_string(),
    }
}

/// Whether a page row counts as changed: any diffed pixels, the page exists on
/// one side only (the whole sheet appeared/disappeared), or its paper size changed
/// (#262 — a resized sheet has no changed pixels to count, but it IS a change).
/// Pure → unit-testable.
pub fn row_changed(presence: Presence, changed_fraction: f64, resized: bool) -> bool {
    presence != Presence::Both || resized || changed_fraction > 0.0
}

/// Display order for the Pages panel: changed pages first, stable by page number
/// within each half — the same rule the layer list uses. Pure → unit-testable.
pub fn changed_first_order(changed: &[bool]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..changed.len()).collect();
    order.sort_by_key(|&i| !changed[i]);
    order
}

/// The bottom-left summary chip text: page totals plus the changed count, with a
/// count mismatch spelled out (an added/removed sheet must never read as "same
/// number of pages"). Pure → unit-testable.
pub fn page_summary(old_pages: usize, new_pages: usize, changed: usize) -> String {
    if old_pages == new_pages {
        format!("{old_pages} page(s) · {changed} changed")
    } else {
        format!("old {old_pages} / new {new_pages} page(s) · {changed} changed")
    }
}

/// World-space bbox `[0, 0, w, h]` (nm) of a page raster: pixels → mm via the
/// known DPI, so the camera, Fit, the grid, and the measure tool all keep working
/// in real millimetres over the page. Pure → unit-testable.
pub fn page_world_bbox(width_px: u32, height_px: u32, dpi: f32) -> [i64; 4] {
    let nm_per_px = 25.4 * etchy_core::NM_PER_MM as f64 / dpi as f64;
    [
        0,
        0,
        (width_px as f64 * nm_per_px).round() as i64,
        (height_px as f64 * nm_per_px).round() as i64,
    ]
}

/// One page of the PDF pair. Raster images are uploaded to GPU textures ONCE (on
/// first draw) and the handles cached — never re-uploaded per frame. The old/new
/// source rasters are dropped after upload; the overlay raster is kept for the
/// per-page PNG export.
pub struct PageRow {
    /// 1-based row number in the merged document (its reading order). Equal to
    /// both sides' page numbers whenever nothing was inserted or removed.
    pub page: usize,
    /// 1-based page in the old PDF, when the sheet exists there.
    pub old_page: Option<usize>,
    /// 1-based page in the new PDF, when the sheet exists there.
    pub new_page: Option<usize>,
    pub presence: Presence,
    pub changed: bool,
    /// `(added+removed+changed) / total` pixels; 1.0 for an unpaired or resized
    /// page (the whole sheet is the change).
    pub changed_fraction: f64,
    /// Framing size in pixels: both sides for a normal pair, and the larger of
    /// the two when the sheet was resized, so the camera frames either side.
    pub width: u32,
    pub height: u32,
    /// Each side's own raster size, so a resized sheet draws both sides at their
    /// true scale — etchy never stretches one raster onto the other's frame.
    pub old_dims: Option<(u32, u32)>,
    pub new_dims: Option<(u32, u32)>,
    /// Set when this sheet's paper size changed between revisions (#262): there
    /// is no pixel overlay and the whole page counts as changed.
    pub size_change: Option<PageSizeChange>,
    /// Set when the two rasters differed only by rasterization rounding and were
    /// cropped to their shared region before diffing (#262). The page keeps its
    /// overlay; this only says which pixels were compared.
    pub rounding_crop: Option<PageRoundingCrop>,
    pub old_img: Option<Image>,
    pub new_img: Option<Image>,
    /// The diff overlay raster (green added / red removed / amber changed on
    /// board-dark), kept after texture upload for PNG export. `None` for an
    /// unpaired sheet and for a resized one (nothing to overlay).
    pub overlay_img: Option<Image>,
    old_tex: Option<egui::TextureHandle>,
    new_tex: Option<egui::TextureHandle>,
    overlay_tex: Option<egui::TextureHandle>,
}

fn upload(ctx: &egui::Context, name: String, img: &Image) -> egui::TextureHandle {
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [img.width as usize, img.height as usize],
        &img.rgba,
    );
    ctx.load_texture(name, ci, egui::TextureOptions::LINEAR)
}

impl PageRow {
    /// Build a row from its rasters. Sizes are read off the images themselves, so
    /// the framing size and each side's true size can never drift apart.
    pub fn new(
        plan: PagePlanRow,
        changed_fraction: f64,
        old_img: Option<Image>,
        new_img: Option<Image>,
        overlay_img: Option<Image>,
        size_change: Option<PageSizeChange>,
        rounding_crop: Option<PageRoundingCrop>,
    ) -> Self {
        let dims = |i: &Option<Image>| i.as_ref().map(|i| (i.width, i.height));
        let (old_dims, new_dims) = (dims(&old_img), dims(&new_img));
        // Frame the larger of the two sides: with a resized sheet the smaller one
        // must still fit, and for a normal pair both are identical.
        let width = old_dims.map_or(0, |d| d.0).max(new_dims.map_or(0, |d| d.0));
        let height = old_dims.map_or(0, |d| d.1).max(new_dims.map_or(0, |d| d.1));
        Self {
            page: plan.row,
            old_page: plan.old_page,
            new_page: plan.new_page,
            presence: plan.presence,
            changed: row_changed(plan.presence, changed_fraction, size_change.is_some()),
            changed_fraction,
            width,
            height,
            old_dims,
            new_dims,
            size_change,
            rounding_crop,
            old_img,
            new_img,
            overlay_img,
            old_tex: None,
            new_tex: None,
            overlay_tex: None,
        }
    }

    /// The row's label — names the page of the revision it belongs to, and says
    /// so when a sheet moved between revisions.
    pub fn label(&self) -> String {
        row_label(self.old_page, self.new_page)
    }

    /// World bbox (nm) of the old-side raster, at its own true scale.
    pub fn old_bbox(&self, dpi: f32) -> Option<[i64; 4]> {
        self.old_dims.map(|(w, h)| page_world_bbox(w, h, dpi))
    }

    /// World bbox (nm) of the new-side raster, at its own true scale.
    pub fn new_bbox(&self, dpi: f32) -> Option<[i64; 4]> {
        self.new_dims.map(|(w, h)| page_world_bbox(w, h, dpi))
    }

    /// World bbox (nm) the camera should frame: the union of the two sides (both
    /// start at the origin), so a resized sheet shows either side whole.
    pub fn fit_bbox(&self, dpi: f32) -> [i64; 4] {
        page_world_bbox(self.width, self.height, dpi)
    }

    /// Old-side texture, uploaded once on first use; the source raster is dropped
    /// after upload (the texture is the copy that matters).
    pub fn old_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.old_tex.is_none() {
            if let Some(img) = self.old_img.take() {
                self.old_tex = Some(upload(ctx, format!("pdf-old-{}", self.page), &img));
            }
        }
        self.old_tex.as_ref().map(|t| t.id())
    }

    /// New-side texture, uploaded once on first use (source raster dropped).
    pub fn new_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.new_tex.is_none() {
            if let Some(img) = self.new_img.take() {
                self.new_tex = Some(upload(ctx, format!("pdf-new-{}", self.page), &img));
            }
        }
        self.new_tex.as_ref().map(|t| t.id())
    }

    /// Diff-overlay texture, uploaded once on first use. The overlay raster is
    /// KEPT (not dropped) — the Export tab encodes it to PNG on demand.
    pub fn overlay_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.overlay_tex.is_none() {
            if let Some(img) = &self.overlay_img {
                self.overlay_tex = Some(upload(ctx, format!("pdf-overlay-{}", self.page), img));
            }
        }
        self.overlay_tex.as_ref().map(|t| t.id())
    }
}

/// The whole PDF-pair view state, held by the app alongside (and mutually
/// exclusive with) the board diff.
pub struct PdfView {
    /// Rasterization DPI — the pixels→mm scale the canvas and measure tool use.
    pub dpi: f32,
    pub old_pages: usize,
    pub new_pages: usize,
    /// One row per page of the pair, in natural page order.
    pub rows: Vec<PageRow>,
    /// Display order for the Pages panel: changed pages first (indices into `rows`).
    pub order: Vec<usize>,
    /// The selected page (index into `rows`).
    pub selected: usize,
    /// The page alignment chosen (#249) — `None` when it was the plain index
    /// pairing. Shown in the viewer whenever it is `Some`: a re-pairing that
    /// silently changed which sheets were compared is not acceptable.
    pub alignment_note: Option<String>,
}

impl PdfView {
    pub fn changed_count(&self) -> usize {
        self.rows.iter().filter(|r| r.changed).count()
    }

    pub fn summary(&self) -> String {
        page_summary(self.old_pages, self.new_pages, self.changed_count())
    }
}

/// Rasterize both PDFs at `dpi` (#223: user-settable in Settings > Diff), diff
/// paired pages, and assemble the view.
///
/// Fails loud on: unparseable PDF, a page over the pixel caps at that DPI (before
/// any rendering), a page count past `etchy_core::MAX_ALIGN_PAGES` (page alignment
/// is quadratic in the page count, so it is refused before allocating), or a page
/// that would rasterize to zero pixels.
///
/// A paired sheet whose paper size **changed** is NOT a failure (#262): it comes
/// back as a wholly-changed row carrying a [`PageSizeChange`], with both sides at
/// their true scale and no overlay. Sizes that differ only by rasterization
/// rounding are the same sheet: both sides are cropped to their shared region and
/// diffed normally, reported as a [`PageRoundingCrop`].
#[cfg(feature = "pdf")]
pub fn build_pdf_view(old: &[u8], new: &[u8], dpi: f32) -> anyhow::Result<PdfView> {
    use anyhow::Context as _;
    // Enforce the pixel ceilings BEFORE rasterizing anything (the CLI's
    // pre-flight plus the GUI-specific per-dimension and whole-document caps —
    // an over-limit texture PANICS in egui_glow and a many-page pair OOMs, so
    // both must fail loud here instead).
    let mut doc_px: u64 = 0;
    for (label, bytes) in [("old", old), ("new", new)] {
        let dims = etchy_pdf::page_pixel_dims(bytes, dpi)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("reading the {label} PDF"))?;
        // Page alignment is a DP matrix quadratic in the page count — refuse a
        // document pair too big for it BEFORE rasterizing a single page (#249).
        if dims.len() > etchy_core::MAX_ALIGN_PAGES {
            anyhow::bail!(
                "the {label} PDF has {} pages, over the {}-page limit etchy will align \
                 (page alignment cost grows with the square of the page count) — split \
                 the document, or diff it with the CLI",
                dims.len(),
                etchy_core::MAX_ALIGN_PAGES
            );
        }
        for (i, (w, h)) in dims.iter().enumerate() {
            let px = u64::from(*w) * u64::from(*h);
            if px == 0 {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}x{h} px at {dpi} DPI — \
                     nothing to compare",
                    i + 1
                );
            }
            if *w > MAX_TEXTURE_SIDE || *h > MAX_TEXTURE_SIDE {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}x{h} px at {dpi} DPI — a side \
                     over the {MAX_TEXTURE_SIDE} px GPU texture limit — lower the PDF \
                     resolution in Settings > Diff (or use the CLI's --dpi)",
                    i + 1
                );
            }
            if px > MAX_PAGE_PIXELS {
                anyhow::bail!(
                    "{label} PDF page {} would rasterize to {w}x{h} px (~{} MP) at {dpi} DPI, \
                     over the ~{} MP per-page cap",
                    i + 1,
                    px / 1_000_000,
                    MAX_PAGE_PIXELS / 1_000_000
                );
            }
            doc_px += px;
        }
    }
    if doc_px > MAX_DOC_PIXELS {
        anyhow::bail!(
            "this PDF pair would rasterize to ~{} MP in total, over the viewer's ~{} MP \
             budget — lower the PDF resolution in Settings > Diff, or diff it with the CLI",
            doc_px / 1_000_000,
            MAX_DOC_PIXELS / 1_000_000
        );
    }
    let old_imgs = etchy_pdf::rasterize(old, dpi)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("rasterizing the old PDF")?;
    let new_imgs = etchy_pdf::rasterize(new, dpi)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("rasterizing the new PDF")?;
    let (old_pages, new_pages) = (old_imgs.len(), new_imgs.len());
    // Pair the sheets by content off the rasters we already hold (#249) — no
    // re-render, and the caps above still bound everything.
    let old_fp: Vec<_> = old_imgs.iter().map(etchy_core::fingerprint).collect();
    let new_fp: Vec<_> = new_imgs.iter().map(etchy_core::fingerprint).collect();
    let alignment =
        etchy_core::align_pages(&old_fp, &new_fp).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut old_imgs: Vec<Option<Image>> = old_imgs.into_iter().map(Some).collect();
    let mut new_imgs: Vec<Option<Image>> = new_imgs.into_iter().map(Some).collect();
    let mut rows = Vec::with_capacity(alignment.matches.len());
    for plan in page_plan(&alignment) {
        // Each index appears in exactly one row (the alignment's accounting
        // invariant), so taking the raster out is safe and avoids a clone.
        let o = plan.old_page.and_then(|p| old_imgs[p - 1].take());
        let n = plan.new_page.and_then(|p| new_imgs[p - 1].take());
        let row = match (o, n) {
            (Some(o), Some(n)) => {
                let dims = |i: &Image| (i.width, i.height);
                // A rounding-sized difference is the SAME sheet: crop to the
                // shared region so the sheet keeps a real, located pixel diff.
                // Only a genuine resize gives up the overlay (#262).
                let (to, rounding_crop) = match etchy_core::classify_pair_size(dims(&o), dims(&n)) {
                    etchy_core::PairSizing::Same => (None, None),
                    etchy_core::PairSizing::Rounded { to } => (
                        Some(to),
                        Some(PageRoundingCrop {
                            old: dims(&o),
                            new: dims(&n),
                            to,
                        }),
                    ),
                    etchy_core::PairSizing::Changed => {
                        // No pixel correspondence: the row is wholly changed
                        // and says so — the load keeps going, and neither
                        // raster is rescaled onto the other's frame.
                        let size_change = PageSizeChange {
                            old: dims(&o),
                            new: dims(&n),
                        };
                        rows.push(PageRow::new(
                            plan,
                            1.0,
                            Some(o),
                            Some(n),
                            None,
                            Some(size_change),
                            None,
                        ));
                        continue;
                    }
                };
                // Paired same-size sheet: ALWAYS the full pixel diff — the
                // fingerprint chose the pairing and nothing else.
                // Cropped copies only when there is something to crop — the
                // common path diffs the rasters in place, no clone.
                let cropped = match to {
                    Some((w, h)) => {
                        let crop = |img: &Image| {
                            etchy_core::crop_top_left(img, w, h).map_err(|e| anyhow::anyhow!("{e}"))
                        };
                        Some((crop(&o)?, crop(&n)?))
                    }
                    None => None,
                };
                let (od, nd) = match &cropped {
                    Some((oc, nc)) => (oc, nc),
                    None => (&o, &n),
                };
                let diff = etchy_core::diff_images(od, nd, &Default::default())
                    .map_err(|e| anyhow::anyhow!("{e}"))
                    .with_context(|| {
                        format!("diffing {}", row_label(plan.old_page, plan.new_page))
                    })?;
                PageRow::new(
                    plan,
                    diff.stats.changed_fraction,
                    Some(o),
                    Some(n),
                    Some(diff.overlay),
                    None,
                    rounding_crop,
                )
            }
            // Unpaired sheet: the whole page appeared or disappeared — carried as
            // an explicit changed row with only its own side's raster.
            (Some(o), None) => PageRow::new(plan, 1.0, Some(o), None, None, None, None),
            (None, Some(n)) => PageRow::new(plan, 1.0, None, Some(n), None, None, None),
            (None, None) => unreachable!("every plan row names at least one side"),
        };
        rows.push(row);
    }
    let changed: Vec<bool> = rows.iter().map(|r| r.changed).collect();
    let order = changed_first_order(&changed);
    let selected = order.first().copied().unwrap_or(0);
    Ok(PdfView {
        dpi,
        old_pages,
        new_pages,
        rows,
        order,
        selected,
        alignment_note: alignment.note(),
    })
}

/// Without the `pdf` feature a PDF input is a loud, actionable error — not a
/// confusing "not a gerber" skip (mirrors the CLI's no-feature path).
#[cfg(not(feature = "pdf"))]
pub fn build_pdf_view(_old: &[u8], _new: &[u8], _dpi: f32) -> anyhow::Result<PdfView> {
    anyhow::bail!("this build lacks PDF support — rebuild etchy-gui with --features pdf")
}

/// Encode a page's diff overlay to PNG bytes for the Export tab.
#[cfg(feature = "pdf")]
pub fn overlay_png(img: &Image) -> anyhow::Result<Vec<u8>> {
    etchy_pdf::encode_png(img).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Unreachable without the feature (no [`PdfView`] can exist to export from),
/// but kept honest rather than panicking.
#[cfg(not(feature = "pdf"))]
pub fn overlay_png(_img: &Image) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("this build lacks PDF support — rebuild etchy-gui with --features pdf")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_magic_detection() {
        assert!(looks_like_pdf(b"%PDF-1.7\nrest"));
        assert!(!looks_like_pdf(b"G04 gerber*"));
        assert!(!looks_like_pdf(b""));
        assert!(!looks_like_pdf(b"  %PDF")); // magic must lead, matching the CLI sniff
    }

    /// Shorthand for an alignment built from `(old, new)` index pairs, where
    /// `None` marks the missing side.
    fn alignment(steps: &[(Option<usize>, Option<usize>)]) -> PageAlignment {
        PageAlignment {
            matches: steps
                .iter()
                .map(|s| match *s {
                    (Some(old), Some(new)) => PageMatch::Paired { old, new },
                    (Some(old), None) => PageMatch::OldOnly { old },
                    (None, Some(new)) => PageMatch::NewOnly { new },
                    (None, None) => unreachable!("a step names at least one side"),
                })
                .collect(),
            pairing: etchy_core::Pairing::Content,
        }
    }

    #[test]
    fn page_plan_carries_both_sides_page_numbers() {
        let plan = page_plan(&alignment(&[(Some(0), Some(0)), (Some(1), Some(1))]));
        assert_eq!(
            plan,
            vec![
                PagePlanRow {
                    row: 1,
                    presence: Presence::Both,
                    old_page: Some(1),
                    new_page: Some(1)
                },
                PagePlanRow {
                    row: 2,
                    presence: Presence::Both,
                    old_page: Some(2),
                    new_page: Some(2)
                },
            ]
        );
        // A trailing append: the excess new sheet is an explicit new-only row.
        let plan = page_plan(&alignment(&[(Some(0), Some(0)), (None, Some(1))]));
        assert_eq!(plan[1].presence, Presence::NewOnly);
        assert_eq!((plan[1].old_page, plan[1].new_page), (None, Some(2)));
        // Removed sheets are old-only rows.
        let plan = page_plan(&alignment(&[(Some(0), Some(0)), (Some(1), None)]));
        assert_eq!(plan[1].presence, Presence::OldOnly);
        assert_eq!((plan[1].old_page, plan[1].new_page), (Some(2), None));
    }

    #[test]
    fn page_plan_keeps_an_inserted_sheet_in_place_and_shifts_the_rest() {
        // #249: old [A, B, C]; new [A, X, B, C]. The inserted sheet is row 2 and
        // the later sheets keep pairing with themselves across the shift.
        let plan = page_plan(&alignment(&[
            (Some(0), Some(0)),
            (None, Some(1)),
            (Some(1), Some(2)),
            (Some(2), Some(3)),
        ]));
        assert_eq!(plan.len(), 4);
        assert_eq!(plan[1].presence, Presence::NewOnly);
        assert_eq!((plan[2].old_page, plan[2].new_page), (Some(2), Some(3)));
        assert_eq!((plan[3].old_page, plan[3].new_page), (Some(3), Some(4)));
        // Every row is numbered by its place in the merged document.
        assert_eq!(plan.iter().map(|r| r.row).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn row_labels_name_the_revision_a_page_number_belongs_to() {
        assert_eq!(row_label(Some(2), Some(2)), "page 2");
        // A sheet that moved says so — the row number is not a page number.
        assert_eq!(row_label(Some(2), Some(3)), "page 3 (was 2)");
        assert_eq!(row_label(Some(4), None), "page 4");
        assert_eq!(row_label(None, Some(1)), "page 1");
    }

    #[test]
    fn unpaired_and_resized_pages_always_count_as_changed() {
        assert!(!row_changed(Presence::Both, 0.0, false));
        assert!(row_changed(Presence::Both, 0.001, false));
        // A sheet appearing or disappearing IS a change, whatever its pixels say.
        assert!(row_changed(Presence::OldOnly, 0.0, false));
        assert!(row_changed(Presence::NewOnly, 0.0, false));
        // And so is a resized sheet, which has no changed pixels to count (#262).
        assert!(row_changed(Presence::Both, 0.0, true));
    }

    #[test]
    fn changed_pages_order_first_and_stable() {
        assert_eq!(
            changed_first_order(&[false, true, false, true]),
            [1, 3, 0, 2]
        );
        assert_eq!(changed_first_order(&[false, false]), [0, 1]);
        assert_eq!(changed_first_order(&[]), Vec::<usize>::new());
    }

    #[test]
    fn summary_spells_out_a_page_count_mismatch() {
        assert_eq!(page_summary(4, 4, 2), "4 page(s) · 2 changed");
        // An added/removed sheet must never read as "same number of pages".
        assert_eq!(page_summary(4, 5, 1), "old 4 / new 5 page(s) · 1 changed");
    }

    #[test]
    fn page_world_bbox_maps_pixels_to_mm_via_dpi() {
        // 150 px at 150 DPI = 1 inch = 25.4 mm = 25_400_000 nm.
        assert_eq!(
            page_world_bbox(150, 300, 150.0),
            [0, 0, 25_400_000, 50_800_000]
        );
        // 100 pt MediaBox rendered at 72 DPI → 100 px → 100/72 inch.
        let bb = page_world_bbox(100, 100, 72.0);
        let want = (100.0 / 72.0 * 25.4e6_f64).round() as i64;
        assert_eq!(bb, [0, 0, want, want]);
    }

    #[test]
    fn presence_tags_read_for_trust() {
        assert_eq!(Presence::Both.tag(), None);
        assert_eq!(Presence::OldOnly.tag(), Some("old only"));
        assert_eq!(Presence::NewOnly.tag(), Some("new only"));
    }
}

/// Test-support: one synthesized sheet — MediaBox size in points and the
/// position of its filled 20x20 black square.
#[cfg(all(test, feature = "pdf"))]
#[derive(Clone, Copy)]
pub(crate) struct TestSheet {
    pub size: (f64, f64),
    pub square: (i32, i32),
}

/// A sheet on the standard 100x100 pt page with its square at (`x`,`y`).
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn test_sheet(x: i32, y: i32) -> TestSheet {
    TestSheet {
        size: (100.0, 100.0),
        square: (x, y),
    }
}

/// A sheet of an arbitrary MediaBox size, in points.
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn test_sized_sheet(w: f64, h: f64, x: i32, y: i32) -> TestSheet {
    TestSheet {
        size: (w, h),
        square: (x, y),
    }
}

/// Test-support (shared with main.rs's export regression test): a minimal
/// well-formed single-page PDF (100x100 pt MediaBox) with one filled black
/// square at (`x`,`y`), size 20 — same synthesis as etchy-pdf's tests.
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn one_square_pdf(x: i32, y: i32) -> Vec<u8> {
    multi_page_pdf(&[test_sheet(x, y)])
}

/// Test-support: a well-formed multi-page PDF, one page per [`TestSheet`].
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn multi_page_pdf(sheets: &[TestSheet]) -> Vec<u8> {
    // Object layout: 1 = catalog, 2 = page tree, then (page, content) pairs.
    let kids: Vec<String> = (0..sheets.len())
        .map(|i| format!("{} 0 R", 3 + i * 2))
        .collect();
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            sheets.len()
        ),
    ];
    for (i, s) in sheets.iter().enumerate() {
        let (x, y) = s.square;
        let content = format!("0 0 0 rg\n{x} {y} 20 20 re\nf\n");
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
             /Contents {} 0 R /Resources << >> >>",
            s.size.0,
            s.size.1,
            4 + i * 2
        ));
        objs.push(format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ));
    }
    let mut pdf = String::from("%PDF-1.7\n");
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
    }
    let xref_pos = pdf.len();
    pdf.push_str(&format!("xref\n0 {}\n", objs.len() + 1));
    pdf.push_str("0000000000 65535 f \n");
    for off in &offsets {
        pdf.push_str(&format!("{off:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_pos}\n%%EOF",
        objs.len() + 1
    ));
    pdf.into_bytes()
}

// End-to-end over the real engine (hayro rasterize + pixel diff) — needs the
// feature; runs in the default test config since `pdf` is a default feature.
#[cfg(all(test, feature = "pdf"))]
mod pdf_tests {
    use super::*;

    #[test]
    fn identical_pdfs_build_an_unchanged_view() {
        let pdf = one_square_pdf(10, 10);
        let v = build_pdf_view(&pdf, &pdf, 150.0).expect("build");
        assert_eq!((v.old_pages, v.new_pages), (1, 1));
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.changed_count(), 0);
        let row = &v.rows[0];
        assert_eq!(row.presence, Presence::Both);
        assert!(!row.changed);
        assert!(row.old_img.is_some() && row.new_img.is_some());
        assert!(row.overlay_img.is_some(), "paired pages carry an overlay");
        // 100 pt MediaBox at 150 DPI → 100/72*150 = 208 px.
        assert_eq!((row.width, row.height), (208, 208));
    }

    #[test]
    fn dpi_scales_the_raster() {
        // The same page rendered at a higher DPI produces a proportionally
        // larger raster (#223: the Settings > Diff DPI chips re-rasterize).
        let pdf = one_square_pdf(10, 10);
        let lo = build_pdf_view(&pdf, &pdf, 150.0).expect("150 dpi");
        let hi = build_pdf_view(&pdf, &pdf, 300.0).expect("300 dpi");
        assert_eq!(hi.dpi, 300.0);
        assert!(hi.rows[0].width > lo.rows[0].width);
        // 2x the DPI is 2x the pixels (within a rounding pixel).
        assert!((hi.rows[0].width as i64 - 2 * lo.rows[0].width as i64).abs() <= 2);
    }

    #[test]
    fn a_dpi_over_the_caps_fails_loud() {
        // 100 pt at 6000 DPI → 8333 px a side: over both the 8192 px texture
        // side cap and the 50 MP page cap. Must fail loud before rasterizing,
        // so the GUI can revert the DPI setting (#223).
        let pdf = one_square_pdf(10, 10);
        let err = match build_pdf_view(&pdf, &pdf, 6000.0) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("an over-cap DPI must fail loud"),
        };
        assert!(err.contains("DPI"), "error names the DPI: {err}");
    }

    #[test]
    fn a_moved_square_marks_the_page_changed() {
        let v =
            build_pdf_view(&one_square_pdf(10, 10), &one_square_pdf(60, 60), 150.0).expect("build");
        assert_eq!(v.changed_count(), 1);
        let row = &v.rows[0];
        assert!(row.changed);
        assert!(row.changed_fraction > 0.0);
        assert_eq!(v.selected, 0, "the changed page is selected on load");
        // The overlay encodes to a valid PNG through the export helper.
        let png = overlay_png(row.overlay_img.as_ref().unwrap()).expect("png");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn junk_bytes_fail_loud() {
        assert!(build_pdf_view(b"not a pdf", b"also not", 150.0).is_err());
    }

    #[test]
    fn a_rounding_sized_pair_keeps_its_overlay_and_says_it_was_cropped() {
        // #262 tolerance: the same A4-landscape sheet whose MediaBox is 842 pt on
        // one side and the exact 841.89 on the other rasterizes a pixel narrower.
        // Comparing dimensions exactly called that a resize and dropped the
        // overlay; it must stay a normally-diffed page, with the crop on screen.
        let old = multi_page_pdf(&[test_sized_sheet(842.0, 595.0, 40, 40)]);
        let new = multi_page_pdf(&[test_sized_sheet(841.9, 595.0, 300, 300)]);
        let v = build_pdf_view(&old, &new, 150.0).expect("build");
        let row = &v.rows[0];
        assert!(row.size_change.is_none(), "rounding is not a resize");
        let crop = row.rounding_crop.expect("the crop is reported");
        assert_eq!(crop.to, (1753, 1239));
        assert!(
            row.overlay_img.is_some(),
            "the sheet keeps its pixel overlay"
        );
        assert!(row.changed, "and the moved square is found");
        assert!(row.changed_fraction > 0.0);
        // The overlay covers the shared region, not either full raster.
        let ov = row.overlay_img.as_ref().unwrap();
        assert_eq!((ov.width, ov.height), crop.to);
        // Nothing is listed as missing an overlay.
        assert!(row.overlay_img.is_some());
        let text = crop.to_string();
        assert!(text.contains("shared 1753x1239"), "{text}");
    }

    #[test]
    fn a_mid_document_insertion_keeps_the_other_sheets_paired() {
        // #249: old [A, B, C]; new [A, X, B, C]. Index pairing made every later
        // sheet read as heavily changed; only the inserted sheet is a change.
        let old = multi_page_pdf(&[test_sheet(10, 10), test_sheet(60, 60), test_sheet(10, 60)]);
        let new = multi_page_pdf(&[
            test_sheet(10, 10),
            test_sheet(60, 10),
            test_sheet(60, 60),
            test_sheet(10, 60),
        ]);
        let v = build_pdf_view(&old, &new, 150.0).expect("build");
        assert_eq!((v.old_pages, v.new_pages), (3, 4));
        assert_eq!(v.rows.len(), 4, "every sheet of both revisions gets a row");
        assert_eq!(v.changed_count(), 1, "only the inserted sheet changed");
        let ins = &v.rows[1];
        assert_eq!(ins.presence, Presence::NewOnly);
        assert_eq!((ins.old_page, ins.new_page), (None, Some(2)));
        assert!(
            ins.overlay_img.is_none(),
            "nothing to diff an insert against"
        );
        // The shifted sheets pair with themselves and read as unchanged.
        assert_eq!((v.rows[2].old_page, v.rows[2].new_page), (Some(2), Some(3)));
        assert!(!v.rows[2].changed);
        assert_eq!(v.rows[2].label(), "page 3 (was 2)");
        // The viewer states the alignment it chose — never a silent re-pairing.
        let note = v.alignment_note.expect("a re-pairing is always reported");
        assert!(note.contains("inserted"), "{note}");
        // Every page of both revisions is accounted for exactly once.
        let olds: Vec<usize> = v.rows.iter().filter_map(|r| r.old_page).collect();
        let news: Vec<usize> = v.rows.iter().filter_map(|r| r.new_page).collect();
        assert_eq!(olds, vec![1, 2, 3]);
        assert_eq!(news, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_resized_sheet_is_a_changed_row_not_a_failed_load() {
        // #262: a sheet whose paper size changed used to fail the whole load with
        // ImageSizeMismatch. It is a legitimate revision diff: the row loads,
        // reads as wholly changed, and says why.
        let old = multi_page_pdf(&[test_sheet(10, 10), test_sheet(60, 60)]);
        let new = multi_page_pdf(&[
            TestSheet {
                size: (100.0, 200.0),
                square: (10, 10),
            },
            test_sheet(60, 60),
        ]);
        let v = build_pdf_view(&old, &new, 150.0).expect("a resized sheet must still load");
        assert_eq!(v.rows.len(), 2);
        let row = &v.rows[0];
        assert_eq!(row.presence, Presence::Both, "the sheet is on both sides");
        assert!(row.changed, "a resized sheet is a change");
        assert_eq!(row.changed_fraction, 1.0, "the whole sheet is the change");
        let sc = row.size_change.expect("the size change is on the row");
        assert_eq!((sc.old, sc.new), ((208, 208), (208, 416)));
        assert!(
            row.overlay_img.is_none(),
            "different dimensions have no pixel overlay"
        );
        assert!(
            row.old_img.is_some() && row.new_img.is_some(),
            "both sides are still viewable"
        );
        // Each side keeps its own world bbox, so neither raster is stretched to
        // fit the other — etchy never rescales a raster.
        assert_ne!(row.old_bbox(v.dpi), row.new_bbox(v.dpi));
        assert_eq!(
            row.fit_bbox(v.dpi),
            page_world_bbox(208, 416, v.dpi),
            "the camera frames the larger of the two sheets"
        );
        // The rest of the document still diffed: page 2 is untouched and clean.
        assert!(!v.rows[1].changed);
        assert!(v.rows[1].overlay_img.is_some());
        assert_eq!(v.changed_count(), 1);
    }

    #[test]
    fn an_unchanged_pair_reports_no_realignment() {
        let pdf = multi_page_pdf(&[test_sheet(10, 10), test_sheet(60, 60)]);
        let v = build_pdf_view(&pdf, &pdf, 150.0).expect("build");
        assert_eq!(v.alignment_note, None, "the identity says nothing extra");
        assert_eq!(v.changed_count(), 0);
        assert_eq!(v.rows[1].label(), "page 2");
    }
}
