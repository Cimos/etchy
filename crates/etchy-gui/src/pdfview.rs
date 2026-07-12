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
use etchy_core::Image;

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

/// The 1-based page numbers of the whole document pair with each page's presence:
/// pages are paired by index up to `min(old, new)`; the excess side's pages are
/// explicit `OldOnly` / `NewOnly` entries. Pure → unit-testable.
pub fn page_plan(old_pages: usize, new_pages: usize) -> Vec<(usize, Presence)> {
    let paired = old_pages.min(new_pages);
    (1..=old_pages.max(new_pages))
        .map(|page| {
            let presence = if page <= paired {
                Presence::Both
            } else if old_pages > new_pages {
                Presence::OldOnly
            } else {
                Presence::NewOnly
            };
            (page, presence)
        })
        .collect()
}

/// Whether a page row counts as changed: any diffed pixels, or the page exists on
/// one side only (the whole sheet appeared/disappeared). Pure → unit-testable.
pub fn row_changed(presence: Presence, changed_fraction: f64) -> bool {
    presence != Presence::Both || changed_fraction > 0.0
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
    /// 1-based page number.
    pub page: usize,
    pub presence: Presence,
    pub changed: bool,
    /// `(added+removed+changed) / total` pixels; 1.0 for an unpaired page.
    pub changed_fraction: f64,
    /// Raster dimensions in pixels (both sides match for paired pages — a size
    /// mismatch fails loud at build time).
    pub width: u32,
    pub height: u32,
    pub old_img: Option<Image>,
    pub new_img: Option<Image>,
    /// The diff overlay raster (green added / red removed / amber changed on
    /// board-dark), kept after texture upload for PNG export.
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
    /// Constructor mirrors the row's fields one-to-one (the images are what push
    /// it over clippy's argument budget — a builder would be pure ceremony here).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        page: usize,
        presence: Presence,
        changed_fraction: f64,
        width: u32,
        height: u32,
        old_img: Option<Image>,
        new_img: Option<Image>,
        overlay_img: Option<Image>,
    ) -> Self {
        Self {
            page,
            presence,
            changed: row_changed(presence, changed_fraction),
            changed_fraction,
            width,
            height,
            old_img,
            new_img,
            overlay_img,
            old_tex: None,
            new_tex: None,
            overlay_tex: None,
        }
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
}

impl PdfView {
    pub fn changed_count(&self) -> usize {
        self.rows.iter().filter(|r| r.changed).count()
    }

    pub fn summary(&self) -> String {
        page_summary(self.old_pages, self.new_pages, self.changed_count())
    }
}

/// Rasterize both PDFs at the default DPI, diff paired pages, and assemble the
/// view. Fails loud on: unparseable PDF, a page over the pixel cap (before any
/// rendering), a paired page whose size changed between revisions, or a page that
/// would rasterize to zero pixels.
#[cfg(feature = "pdf")]
pub fn build_pdf_view(old: &[u8], new: &[u8]) -> anyhow::Result<PdfView> {
    use anyhow::Context as _;
    let dpi = etchy_pdf::DEFAULT_DPI;
    // Enforce the pixel ceilings BEFORE rasterizing anything (the CLI's
    // pre-flight plus the GUI-specific per-dimension and whole-document caps —
    // an over-limit texture PANICS in egui_glow and a many-page pair OOMs, so
    // both must fail loud here instead).
    let mut doc_px: u64 = 0;
    for (label, bytes) in [("old", old), ("new", new)] {
        let dims = etchy_pdf::page_pixel_dims(bytes, dpi)
            .map_err(|e| anyhow::anyhow!("{e}"))
            .with_context(|| format!("reading the {label} PDF"))?;
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
                     over the {MAX_TEXTURE_SIDE} px GPU texture limit; diff this pair with \
                     the CLI at a lower --dpi",
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
             budget — diff it with the CLI (per-page overlay PNGs) instead",
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
    let mut old_it = old_imgs.into_iter();
    let mut new_it = new_imgs.into_iter();
    let mut rows = Vec::with_capacity(old_pages.max(new_pages));
    for (page, presence) in page_plan(old_pages, new_pages) {
        let (o, n) = (old_it.next(), new_it.next());
        let row = match (o, n) {
            (Some(o), Some(n)) => {
                // Paired page: the pixel diff. A size mismatch (a resized sheet)
                // fails loud — that is itself a change to flag, never a rescale.
                let diff = etchy_core::diff_images(&o, &n, &Default::default())
                    .map_err(|e| anyhow::anyhow!("{e}"))
                    .with_context(|| format!("diffing page {page}"))?;
                let (w, h) = (o.width, o.height);
                PageRow::new(
                    page,
                    presence,
                    diff.stats.changed_fraction,
                    w,
                    h,
                    Some(o),
                    Some(n),
                    Some(diff.overlay),
                )
            }
            // Unpaired page: the whole sheet appeared or disappeared — carried as
            // an explicit changed row with only its own side's raster.
            (Some(o), None) => {
                let (w, h) = (o.width, o.height);
                PageRow::new(page, presence, 1.0, w, h, Some(o), None, None)
            }
            (None, Some(n)) => {
                let (w, h) = (n.width, n.height);
                PageRow::new(page, presence, 1.0, w, h, None, Some(n), None)
            }
            (None, None) => unreachable!("page_plan never exceeds max(old, new)"),
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
    })
}

/// Without the `pdf` feature a PDF input is a loud, actionable error — not a
/// confusing "not a gerber" skip (mirrors the CLI's no-feature path).
#[cfg(not(feature = "pdf"))]
pub fn build_pdf_view(_old: &[u8], _new: &[u8]) -> anyhow::Result<PdfView> {
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

    #[test]
    fn page_plan_pairs_by_index_and_tags_the_excess() {
        assert_eq!(
            page_plan(2, 2),
            vec![(1, Presence::Both), (2, Presence::Both)]
        );
        // New grew by one page → page 3 is new-only.
        assert_eq!(
            page_plan(2, 3),
            vec![
                (1, Presence::Both),
                (2, Presence::Both),
                (3, Presence::NewOnly)
            ]
        );
        // Old had more → the excess is old-only (removed sheets).
        assert_eq!(
            page_plan(3, 1),
            vec![
                (1, Presence::Both),
                (2, Presence::OldOnly),
                (3, Presence::OldOnly)
            ]
        );
    }

    #[test]
    fn unpaired_pages_always_count_as_changed() {
        assert!(!row_changed(Presence::Both, 0.0));
        assert!(row_changed(Presence::Both, 0.001));
        // A sheet appearing or disappearing IS a change, whatever its pixels say.
        assert!(row_changed(Presence::OldOnly, 0.0));
        assert!(row_changed(Presence::NewOnly, 0.0));
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

// End-to-end over the real engine (hayro rasterize + pixel diff) — needs the
// feature; runs in the default test config since `pdf` is a default feature.
#[cfg(all(test, feature = "pdf"))]
mod pdf_tests {
    use super::*;

    /// Minimal well-formed single-page PDF (100x100 pt MediaBox) with one filled
    /// black square at (`x`,`y`), size 20 — same synthesis as etchy-pdf's tests.
    fn one_square_pdf(x: i32, y: i32) -> Vec<u8> {
        let content = format!("0 0 0 rg\n{x} {y} 20 20 re\nf\n");
        let objs: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] \
              /Contents 4 0 R /Resources << >> >>"
                .into(),
            format!(
                "<< /Length {} >>\nstream\n{content}endstream",
                content.len()
            ),
        ];
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

    #[test]
    fn identical_pdfs_build_an_unchanged_view() {
        let pdf = one_square_pdf(10, 10);
        let v = build_pdf_view(&pdf, &pdf).expect("build");
        assert_eq!((v.old_pages, v.new_pages), (1, 1));
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.changed_count(), 0);
        let row = &v.rows[0];
        assert_eq!(row.presence, Presence::Both);
        assert!(!row.changed);
        assert!(row.old_img.is_some() && row.new_img.is_some());
        assert!(row.overlay_img.is_some(), "paired pages carry an overlay");
        // 100 pt MediaBox at the default 150 DPI → 100/72*150 = 208 px.
        assert_eq!((row.width, row.height), (208, 208));
    }

    #[test]
    fn a_moved_square_marks_the_page_changed() {
        let v = build_pdf_view(&one_square_pdf(10, 10), &one_square_pdf(60, 60)).expect("build");
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
        assert!(build_pdf_view(b"not a pdf", b"also not").is_err());
    }
}
