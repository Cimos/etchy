//! etchy-pdf — schematic-PDF page-by-page pixel diff.
//!
//! Feature-gated behind `pdf` so the pure-Rust PDF stack (hayro + friends) and its
//! higher MSRV stay out of the default geometric-diff binaries. With the feature
//! off, [`available`] returns `false` and nothing heavy is compiled.
//!
//! With `--features pdf`: [`rasterize`] renders each page of a PDF to an RGBA
//! [`etchy_core::Image`] via hayro (no C++/PDFium), and [`diff_pdfs`] pairs the two
//! revisions' pages and runs `etchy_core`'s raster diff on each pair.
//!
//! Pages pair by **content**, not by index (#249): each rasterized page is
//! digested to a cheap fingerprint (`etchy_core::pagealign`) and the two
//! sequences are aligned, so a sheet inserted or removed mid-document becomes an
//! explicit inserted / removed row instead of desynchronising every later pair.
//! The alignment chosen is always reported ([`PdfDiff::alignment`]) — it decides
//! only *which* pages pair, never whether a pair is diffed: every paired sheet
//! gets the full pixel diff, because a fingerprint is lossy and acting on one
//! would be a silent miss. A re-pairing is adopted only when it clearly beats
//! plain index pairing; otherwise the alignment falls back to index pairing and
//! says so, because index pairing is the baseline this must never do worse than.
//!
//! A paired sheet whose **paper size genuinely changed** is a diff, not an error
//! (#262): its two rasters have no pixel correspondence, so it comes back as a
//! wholly changed page carrying a [`SizeChange`] instead of failing the run.
//! Sizes that differ only by rasterization rounding (a couple of pixels per axis,
//! `etchy_core::SIZE_TOLERANCE_PX`) are the *same* sheet: both sides are cropped
//! to their shared region, the pair is pixel-diffed normally, and the crop is
//! reported ([`RoundingCrop`]). Errors are reserved for input that cannot be
//! rendered at all, or a page count past `etchy_core::MAX_ALIGN_PAGES`.

/// Whether the PDF backend is compiled into this build.
#[cfg(feature = "pdf")]
pub fn available() -> bool {
    true
}

/// Whether the PDF backend is compiled into this build.
#[cfg(not(feature = "pdf"))]
pub fn available() -> bool {
    false
}

#[cfg(feature = "pdf")]
mod imp {
    use etchy_core::{
        align_pages, classify_pair_size, crop_top_left, diff_images, fingerprint, EngineError,
        Image, ImageDiffOptions, ImageDiffResult, PageAlignment, PageMatch, PairSizing,
    };
    use hayro::hayro_interpret::hayro_syntax::Pdf;
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::render_pdf;

    /// PDF-diff failures, distinct from the engine's geometry errors.
    #[derive(Debug)]
    pub enum PdfError {
        /// hayro could not parse the bytes as a PDF.
        Load(String),
        /// A PDF parsed but produced no rasterizable pages.
        NoPages,
        /// The raster engine rejected a page pair (a malformed buffer), or the
        /// documents have more pages than page alignment will allocate for
        /// (`EngineError::TooManyPages`). A page whose paper size changed is NOT
        /// an error — see [`SizeChange`].
        Engine(EngineError),
        /// PNG encoding of an overlay failed.
        Encode(String),
    }

    impl std::fmt::Display for PdfError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                PdfError::Load(e) => write!(f, "could not read PDF: {e}"),
                PdfError::NoPages => write!(f, "PDF has no rasterizable pages"),
                PdfError::Engine(e) => write!(f, "{e}"),
                PdfError::Encode(e) => write!(f, "PNG encode failed: {e}"),
            }
        }
    }
    impl std::error::Error for PdfError {}
    impl From<EngineError> for PdfError {
        fn from(e: EngineError) -> Self {
            PdfError::Engine(e)
        }
    }

    /// Default rasterization resolution. 150 DPI is a good legibility/size trade for
    /// schematic sheets; the pixel diff scales linearly with it.
    pub const DEFAULT_DPI: f32 = 150.0;
    const PDF_POINTS_PER_INCH: f32 = 72.0;

    /// Render every page of `bytes` to an RGBA [`Image`] at `dpi`.
    pub fn rasterize(bytes: &[u8], dpi: f32) -> Result<Vec<Image>, PdfError> {
        let pdf = Pdf::new(bytes.to_vec()).map_err(|e| PdfError::Load(format!("{e:?}")))?;
        let scale = dpi / PDF_POINTS_PER_INCH;
        let pixmaps = render_pdf(&pdf, scale, InterpreterSettings::default(), None)
            .ok_or(PdfError::NoPages)?;
        if pixmaps.is_empty() {
            return Err(PdfError::NoPages);
        }
        let mut out = Vec::with_capacity(pixmaps.len());
        for pm in pixmaps {
            let (w, h) = (pm.width() as u32, pm.height() as u32);
            // hayro renders on white (see its render settings); take straight (un-
            // premultiplied) RGBA so the engine's luma/ink test sees true colours.
            let rgba: Vec<u8> = pm
                .take_unpremultiplied()
                .into_iter()
                .flat_map(|p| p.to_u8_array())
                .collect();
            out.push(Image::new(w, h, rgba)?);
        }
        Ok(out)
    }

    /// Pixel dimensions each page would rasterize to at `dpi`, **without rendering**.
    /// Lets the caller enforce a per-page memory ceiling before committing to a big
    /// raster (a large sheet at high DPI is a DoS-sized allocation otherwise).
    /// Mirrors the sizing in [`rasterize`] (`render_dimensions` × scale, floored).
    pub fn page_pixel_dims(bytes: &[u8], dpi: f32) -> Result<Vec<(u32, u32)>, PdfError> {
        let pdf = Pdf::new(bytes.to_vec()).map_err(|e| PdfError::Load(format!("{e:?}")))?;
        let scale = dpi / PDF_POINTS_PER_INCH;
        let pages = pdf.pages();
        if pages.is_empty() {
            return Err(PdfError::NoPages);
        }
        Ok(pages
            .iter()
            .map(|p| {
                let (w, h) = p.render_dimensions();
                ((w * scale).floor() as u32, (h * scale).floor() as u32)
            })
            .collect())
    }

    /// A paired sheet whose two sides rasterized to genuinely different pixel
    /// dimensions — the sheet's paper size changed between revisions (#262).
    /// Dimensions in pixels at the diff's DPI, which is proportional to the paper
    /// size. A difference of only a pixel or two per axis is rasterization
    /// rounding, not this — see [`RoundingCrop`].
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SizeChange {
        pub old: (u32, u32),
        pub new: (u32, u32),
    }

    impl std::fmt::Display for SizeChange {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "{}x{} px -> {}x{} px",
                self.old.0, self.old.1, self.new.0, self.new.1
            )
        }
    }

    /// A paired sheet whose two rasters differed only by sub-pixel rasterization
    /// rounding (within `etchy_core::SIZE_TOLERANCE_PX` per axis). It is the SAME
    /// sheet size, so both sides were cropped to `to` and pixel-diffed normally —
    /// the alternative, calling it a resize, throws the sheet's whole diff away.
    /// Reported so the few cropped pixels are never an unexplained gap.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct RoundingCrop {
        pub old: (u32, u32),
        pub new: (u32, u32),
        /// The shared region both sides were cropped to before diffing.
        pub to: (u32, u32),
    }

    impl std::fmt::Display for RoundingCrop {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "{}x{} px / {}x{} px, diffed over the shared {}x{} px",
                self.old.0, self.old.1, self.new.0, self.new.1, self.to.0, self.to.1
            )
        }
    }

    /// One row of the merged document: a paired sheet with its pixel diff, or a
    /// sheet that exists in only one revision.
    pub struct PageDiff {
        /// 1-based row number in the merged document (the order a reader sees).
        pub page: usize,
        /// 1-based page number in the old PDF, when the sheet exists there.
        pub old_page: Option<usize>,
        /// 1-based page number in the new PDF, when the sheet exists there.
        pub new_page: Option<usize>,
        /// The full pixel diff of a paired sheet. `None` for a sheet that exists
        /// on one side only (nothing to diff against) or a paired sheet whose
        /// paper size changed (the two rasters have different dimensions, so
        /// there is no pixel correspondence to diff).
        pub diff: Option<ImageDiffResult>,
        /// Set when a paired sheet's paper size changed between revisions (#262):
        /// the page is reported as wholly changed rather than failing the run.
        pub size_change: Option<SizeChange>,
        /// Set when the two rasters differed only by rasterization rounding and
        /// were cropped to their shared region before diffing (#262). The sheet
        /// still has a real pixel diff and a real overlay.
        pub rounding_crop: Option<RoundingCrop>,
    }

    impl PageDiff {
        /// Whether both revisions carry this sheet.
        pub fn is_paired(&self) -> bool {
            self.old_page.is_some() && self.new_page.is_some()
        }

        /// Whether this row is a change. A row with no pixel diff is a change by
        /// construction (a sheet appeared or disappeared), and a diffed row uses
        /// raw presence ([`etchy_core::ImageDiffStats::has_any_change`]) rather
        /// than `changed_fraction`, so a change hidden by the noise floor still
        /// counts — the floor never causes a silent "no differences".
        pub fn has_any_change(&self) -> bool {
            match &self.diff {
                Some(d) => d.stats.has_any_change(),
                None => true,
            }
        }
    }

    /// The whole-document diff: one row per sheet of the merged document, each
    /// side's page count, and the page alignment that produced the pairing.
    pub struct PdfDiff {
        pub old_pages: usize,
        pub new_pages: usize,
        /// Every sheet of both revisions, exactly once, in merged reading order.
        pub pages: Vec<PageDiff>,
        /// How the two revisions' pages were paired (#249). Callers must surface
        /// [`PageAlignment::note`] whenever it is `Some` — a re-pairing that
        /// silently changed which sheets were compared is not acceptable.
        pub alignment: PageAlignment,
    }

    impl PdfDiff {
        /// Any pixel change on any paired sheet, any inserted/removed sheet, or a
        /// page-count change.
        pub fn any_changes(&self) -> bool {
            self.old_pages != self.new_pages
                || !self.alignment.is_identity()
                || self.pages.iter().any(|p| p.has_any_change())
        }
    }

    /// Rasterize both PDFs at `dpi`, align their pages by content, and pixel-diff
    /// every paired sheet in full.
    ///
    /// - Sheets that exist on one side only come back as explicit rows with no
    ///   diff — a sheet appearing or disappearing is a change, never a skip.
    /// - A pair whose rasters differ by no more than
    ///   `etchy_core::SIZE_TOLERANCE_PX` per axis is the same sheet size: both are
    ///   cropped to their shared region, diffed in full, and the crop is reported
    ///   as a [`RoundingCrop`] (#262).
    /// - A pair whose paper size genuinely changed is a **fully changed page**
    ///   carrying a [`SizeChange`], not an error: the run reports it and keeps
    ///   going, and the rest of the document still diffs (#262).
    ///
    /// # Errors
    /// [`PdfError::Load`] / [`PdfError::NoPages`] for input that cannot be
    /// rendered, and [`PdfError::Engine`] for a malformed raster or a page count
    /// past `etchy_core::MAX_ALIGN_PAGES` (page alignment is quadratic in the page
    /// count, so it fails loud before allocating).
    pub fn diff_pdfs(
        old: &[u8],
        new: &[u8],
        dpi: f32,
        opts: &ImageDiffOptions,
    ) -> Result<PdfDiff, PdfError> {
        let old_imgs = rasterize(old, dpi)?;
        let new_imgs = rasterize(new, dpi)?;
        // Fingerprints come off the rasters we already hold — no re-render, and
        // the raster caps the caller enforced still bound everything here.
        let old_fp: Vec<_> = old_imgs.iter().map(fingerprint).collect();
        let new_fp: Vec<_> = new_imgs.iter().map(fingerprint).collect();
        let alignment = align_pages(&old_fp, &new_fp)?;
        let mut pages = Vec::with_capacity(alignment.matches.len());
        for (row, step) in alignment.matches.iter().enumerate() {
            let page = row + 1;
            pages.push(match *step {
                PageMatch::Paired { old, new } => {
                    let (o, n) = (&old_imgs[old], &new_imgs[new]);
                    let dims = |i: &Image| (i.width, i.height);
                    let (mut size_change, mut rounding_crop) = (None, None);
                    // A sheet whose paper size genuinely changed has no pixel
                    // correspondence to diff (#262) — report it as a wholly
                    // changed page with the two sizes named, and keep going.
                    // Exit 2 is reserved for input we cannot render at all. A
                    // rounding-sized difference is the SAME sheet: crop to the
                    // shared region so the real change is still located.
                    let diff = match classify_pair_size(dims(o), dims(n)) {
                        PairSizing::Same => Some(diff_images(o, n, opts)?),
                        PairSizing::Rounded { to } => {
                            rounding_crop = Some(RoundingCrop {
                                old: dims(o),
                                new: dims(n),
                                to,
                            });
                            let (oc, nc) =
                                (crop_top_left(o, to.0, to.1)?, crop_top_left(n, to.0, to.1)?);
                            Some(diff_images(&oc, &nc, opts)?)
                        }
                        PairSizing::Changed => {
                            size_change = Some(SizeChange {
                                old: dims(o),
                                new: dims(n),
                            });
                            None
                        }
                    };
                    PageDiff {
                        page,
                        old_page: Some(old + 1),
                        new_page: Some(new + 1),
                        // Otherwise ALWAYS the full pixel diff: the fingerprint
                        // chose the pairing and nothing more. Matching digests
                        // never skip this.
                        diff,
                        size_change,
                        rounding_crop,
                    }
                }
                PageMatch::OldOnly { old } => PageDiff {
                    page,
                    old_page: Some(old + 1),
                    new_page: None,
                    diff: None,
                    size_change: None,
                    rounding_crop: None,
                },
                PageMatch::NewOnly { new } => PageDiff {
                    page,
                    old_page: None,
                    new_page: Some(new + 1),
                    diff: None,
                    size_change: None,
                    rounding_crop: None,
                },
            });
        }
        Ok(PdfDiff {
            old_pages: old_imgs.len(),
            new_pages: new_imgs.len(),
            pages,
            alignment,
        })
    }

    /// Encode an RGBA [`Image`] to PNG bytes (for writing an overlay to disk).
    pub fn encode_png(img: &Image) -> Result<Vec<u8>, PdfError> {
        let mut buf = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut buf, img.width, img.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc
                .write_header()
                .map_err(|e| PdfError::Encode(e.to_string()))?;
            writer
                .write_image_data(&img.rgba)
                .map_err(|e| PdfError::Encode(e.to_string()))?;
        }
        Ok(buf)
    }
}

#[cfg(feature = "pdf")]
pub use imp::{
    diff_pdfs, encode_png, page_pixel_dims, rasterize, PageDiff, PdfDiff, PdfError, RoundingCrop,
    SizeChange, DEFAULT_DPI,
};

#[cfg(test)]
mod tests {
    #[test]
    fn availability_matches_feature() {
        assert_eq!(super::available(), cfg!(feature = "pdf"));
    }
}

// PDF-backed tests only compile with the feature (they need hayro).
#[cfg(all(test, feature = "pdf"))]
mod pdf_tests {
    use super::imp::*;
    use etchy_core::ImageDiffOptions;

    /// Build a minimal, well-formed single-page PDF (100×100 pt MediaBox) with one
    /// filled black square at (`x`,`y`), size 20. Offsets in the xref table are
    /// computed from the actual byte positions so hayro parses it straight.
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

    /// The pixel diff of a row the test expects to be paired.
    fn paired(p: &PageDiff) -> &etchy_core::ImageDiffResult {
        p.diff.as_ref().expect("row is a paired sheet")
    }

    /// A sheet spec for [`multi_page_pdf`]: the MediaBox size in points, the
    /// filled 20×20 square's position, and its grey level (0 = black).
    #[derive(Clone, Copy)]
    struct Sheet {
        size: (f64, f64),
        square: (i32, i32),
        grey: f64,
    }

    /// A sheet on the standard 100×100 pt page with its black square at (`x`,`y`).
    fn sheet(x: i32, y: i32) -> Sheet {
        Sheet {
            size: (100.0, 100.0),
            square: (x, y),
            grey: 0.0,
        }
    }

    /// A sheet of an arbitrary MediaBox size, in points.
    fn sized_sheet(w: f64, h: f64, x: i32, y: i32) -> Sheet {
        Sheet {
            size: (w, h),
            square: (x, y),
            grey: 0.0,
        }
    }

    /// Build a well-formed multi-page PDF, one `Sheet` per page. Offsets in the
    /// xref table come from the real byte positions so hayro parses it straight.
    fn multi_page_pdf(sheets: &[Sheet]) -> Vec<u8> {
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
            let g = s.grey;
            let content = format!("{g} {g} {g} rg\n{x} {y} 20 20 re\nf\n");
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

    #[test]
    fn a_mid_document_insertion_does_not_desync_the_later_pages() {
        // #249: old [A, B, C]; new [A, X, B, C] — one sheet inserted at page 2.
        // Index pairing compares B↔X and C↔B and reports every later sheet as
        // heavily changed, drowning the real change. Content alignment must pair
        // A↔A, B↔B, C↔C and report X as an inserted sheet.
        let old = multi_page_pdf(&[sheet(10, 10), sheet(60, 60), sheet(10, 60)]);
        let new = multi_page_pdf(&[sheet(10, 10), sheet(60, 10), sheet(60, 60), sheet(10, 60)]);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert!(d.any_changes(), "an inserted sheet is a change");
        let mis_paired = d
            .pages
            .iter()
            .filter(|p| p.diff.as_ref().is_some_and(|r| r.stats.has_any_change()))
            .count();
        assert_eq!(
            mis_paired, 0,
            "the sheets that did not change must pair with themselves"
        );
        // Four rows: the three original sheets, paired, plus the inserted one.
        assert_eq!(d.pages.len(), 4);
        assert_eq!(d.alignment.inserted_pages(), vec![2], "new page 2 inserted");
        assert!(d.alignment.removed_pages().is_empty());
        let ins = &d.pages[1];
        assert_eq!((ins.old_page, ins.new_page), (None, Some(2)));
        assert!(!ins.is_paired() && ins.has_any_change() && ins.diff.is_none());
        // The later sheets pair with themselves, one page number apart.
        assert_eq!(
            (d.pages[2].old_page, d.pages[2].new_page),
            (Some(2), Some(3))
        );
        assert_eq!(
            (d.pages[3].old_page, d.pages[3].new_page),
            (Some(3), Some(4))
        );
        // And the alignment is stated, never silent.
        let note = d.alignment.note().expect("a re-pairing is always reported");
        assert!(note.contains("inserted"), "{note}");
    }

    #[test]
    fn a_paper_size_change_is_a_fully_changed_page_not_an_error() {
        // #262: a sheet resized between revisions is a legitimate revision diff,
        // not a tool malfunction. It must come back as a fully-changed page (the
        // run exits 1), never fail the whole document with exit 2.
        let old = multi_page_pdf(&[sheet(10, 10)]);
        let new = multi_page_pdf(&[sized_sheet(100.0, 200.0, 10, 10)]);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default())
            .expect("a resized sheet is a diff, not an error");
        assert!(d.any_changes(), "a resized sheet is a change");
        let row = &d.pages[0];
        assert!(row.is_paired(), "the sheet still pairs with its original");
        assert!(row.has_any_change());
        let sc = row.size_change.expect("the size change is reported");
        assert_eq!((sc.old, sc.new), ((100, 100), (100, 200)));
        assert!(
            row.diff.is_none(),
            "different dimensions cannot be pixel-diffed"
        );
    }

    #[test]
    fn a_one_pixel_rounding_difference_still_produces_a_located_diff() {
        // #262 tolerance: the SAME A4-landscape sheet, MediaBox [0 0 842 595]
        // against the exact [0 0 841.89 595.276], rasterizes to 1754x1239 vs
        // 1753x1240 px at 150 DPI. Comparing dimensions exactly called that a
        // paper-size change and threw the sheet's whole pixel diff away — the
        // added part was never located. It must be diffed over the shared region.
        let old = multi_page_pdf(&[sized_sheet(842.0, 595.0, 40, 40)]);
        let new = multi_page_pdf(&[sized_sheet(841.89, 595.276, 300, 300)]);
        let od = page_pixel_dims(&old, 150.0).unwrap()[0];
        let nd = page_pixel_dims(&new, 150.0).unwrap()[0];
        assert_ne!(od, nd, "the fixture really does round differently");
        assert!(
            od.0.abs_diff(nd.0) <= 2 && od.1.abs_diff(nd.1) <= 2,
            "{od:?} {nd:?}"
        );
        let d = diff_pdfs(&old, &new, 150.0, &ImageDiffOptions::default()).expect("diff");
        let row = &d.pages[0];
        assert!(
            row.size_change.is_none(),
            "sub-pixel rounding is not a paper-size change: {:?}",
            row.size_change
        );
        let crop = row
            .rounding_crop
            .expect("the crop is reported, never silent");
        assert_eq!(crop.to, (od.0.min(nd.0), od.1.min(nd.1)));
        let s = &paired(row).stats;
        assert!(s.added_px > 100, "the moved square is located: {s:?}");
        assert!(s.removed_px > 100, "and its old position too: {s:?}");
        assert!(s.regions >= 2, "two disjoint regions, got {}", s.regions);
        assert!(d.any_changes());
        // And the crop line names both sizes and the shared region.
        let text = crop.to_string();
        assert!(
            text.contains("shared") && text.contains(&crop.to.0.to_string()),
            "{text}"
        );
    }

    #[test]
    fn a_rounding_difference_on_an_unchanged_sheet_is_still_no_change() {
        // The tolerance must not manufacture a diff either. A MediaBox width of
        // 841.9 pt instead of 842 rounds the raster a pixel narrower without
        // moving any content, so cropping to the shared region must report
        // exactly no change — not a 100%-changed resized sheet, and not an error.
        let old = multi_page_pdf(&[sized_sheet(842.0, 595.0, 40, 40)]);
        let new = multi_page_pdf(&[sized_sheet(841.9, 595.0, 40, 40)]);
        let d = diff_pdfs(&old, &new, 150.0, &ImageDiffOptions::default()).expect("diff");
        let row = &d.pages[0];
        let crop = row.rounding_crop.expect("the crop still applies");
        assert_eq!(crop.to, (1753, 1239));
        assert!(row.size_change.is_none());
        assert!(
            !row.has_any_change(),
            "same content, no change: {:?}",
            paired(row).stats
        );
        assert!(!d.any_changes());
    }

    #[test]
    fn a_size_change_does_not_stop_the_other_pages_diffing() {
        // The rest of the document must still be diffed — one resized sheet does
        // not take the run down with it.
        let old = multi_page_pdf(&[sheet(10, 10), sheet(60, 60)]);
        let new = multi_page_pdf(&[sized_sheet(100.0, 200.0, 10, 10), sheet(10, 60)]);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert_eq!(d.pages.len(), 2);
        assert!(d.pages[0].size_change.is_some());
        assert!(
            d.pages[1].diff.is_some(),
            "page 2 is still pixel-diffed: {:?}",
            d.pages[1].size_change
        );
        assert!(d.pages[1].has_any_change(), "and its change is reported");
    }

    #[test]
    fn a_removed_sheet_mid_document_realigns_the_rest() {
        let old = multi_page_pdf(&[sheet(10, 10), sheet(60, 10), sheet(60, 60)]);
        let new = multi_page_pdf(&[sheet(10, 10), sheet(60, 60)]);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert_eq!(d.alignment.removed_pages(), vec![2]);
        assert_eq!(d.pages.len(), 3);
        assert_eq!((d.pages[1].old_page, d.pages[1].new_page), (Some(2), None));
        assert_eq!(
            (d.pages[2].old_page, d.pages[2].new_page),
            (Some(3), Some(2))
        );
        assert!(
            !d.pages[2]
                .diff
                .as_ref()
                .expect("paired")
                .stats
                .has_any_change(),
            "the surviving sheet pairs with itself"
        );
    }

    #[test]
    fn every_page_of_both_revisions_is_accounted_for_exactly_once() {
        // Trust invariant: the alignment may never drop a sheet. Whatever it
        // chooses, each side's pages come out complete and in order.
        let old = multi_page_pdf(&[sheet(10, 10), sheet(60, 10), sheet(60, 60), sheet(10, 60)]);
        let new = multi_page_pdf(&[sheet(30, 30), sheet(10, 10), sheet(60, 60)]);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        let olds: Vec<usize> = d.pages.iter().filter_map(|p| p.old_page).collect();
        let news: Vec<usize> = d.pages.iter().filter_map(|p| p.new_page).collect();
        assert_eq!(olds, vec![1, 2, 3, 4], "every old page, once, in order");
        assert_eq!(news, vec![1, 2, 3], "every new page, once, in order");
        // Row numbers are 1-based and contiguous over the merged document.
        let rows: Vec<usize> = d.pages.iter().map(|p| p.page).collect();
        assert_eq!(rows, (1..=d.pages.len()).collect::<Vec<_>>());
    }

    #[test]
    fn a_matching_fingerprint_never_skips_the_pixel_diff() {
        // The fingerprint is a 16x16 ink-coverage digest — lossy by construction.
        // Recolouring a part changes no cell's coverage at all, so the two sheets
        // digest IDENTICALLY; they must still be diffed pixel by pixel, or the
        // recolour becomes a silent miss.
        let old = multi_page_pdf(&[sheet(10, 10)]);
        let new = multi_page_pdf(&[Sheet {
            grey: 0.2,
            ..sheet(10, 10)
        }]);
        let (of, nf) = (
            etchy_core::fingerprint(&rasterize(&old, 72.0).unwrap()[0]),
            etchy_core::fingerprint(&rasterize(&new, 72.0).unwrap()[0]),
        );
        assert_eq!(of, nf, "a recolour is invisible to an ink-coverage digest");
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert!(
            d.any_changes(),
            "the pixel diff must still run on an identically-digested pair"
        );
        assert!(d.pages[0].has_any_change());
    }

    #[test]
    fn an_unchanged_multi_page_document_reports_no_realignment() {
        // The quiet path: same sheets, same order — the alignment is the identity
        // and says nothing extra, so the report reads exactly as it always did.
        let pdf = multi_page_pdf(&[sheet(10, 10), sheet(60, 10), sheet(60, 60)]);
        let d = diff_pdfs(&pdf, &pdf, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert!(!d.any_changes());
        assert!(d.alignment.is_identity());
        assert_eq!(d.alignment.note(), None);
        assert_eq!(d.pages.len(), 3);
        assert!(d.pages.iter().all(|p| p.is_paired()));
    }

    #[test]
    fn rasterizes_a_page_to_expected_pixels() {
        let pdf = one_square_pdf(10, 10);
        let imgs = rasterize(&pdf, 72.0).expect("rasterize");
        assert_eq!(imgs.len(), 1, "one page");
        // 100pt MediaBox at 72 DPI → 100×100 px.
        assert_eq!((imgs[0].width, imgs[0].height), (100, 100));
    }

    #[test]
    fn identical_pdfs_show_no_change() {
        let pdf = one_square_pdf(10, 10);
        let d = diff_pdfs(&pdf, &pdf, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert_eq!(d.old_pages, 1);
        assert_eq!(d.new_pages, 1);
        assert!(!d.any_changes(), "a PDF against itself has no diff");
        assert_eq!(paired(&d.pages[0]).stats.changed_fraction, 0.0);
    }

    #[test]
    fn a_moved_square_is_detected_as_added_and_removed() {
        // Same square, different position → old location removed, new location added.
        let old = one_square_pdf(10, 10);
        let new = one_square_pdf(60, 60);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert!(d.any_changes());
        let s = &paired(&d.pages[0]).stats;
        assert!(s.added_px > 100, "new square painted, got {}", s.added_px);
        assert!(s.removed_px > 100, "old square gone, got {}", s.removed_px);
        assert!(s.regions >= 2, "two disjoint squares, got {}", s.regions);
    }

    #[test]
    fn page_pixel_dims_match_rasterized_size_without_rendering() {
        let pdf = one_square_pdf(10, 10);
        // 100 pt MediaBox: 72 DPI → 100×100 px; 144 DPI → 200×200 px.
        assert_eq!(page_pixel_dims(&pdf, 72.0).unwrap(), vec![(100, 100)]);
        assert_eq!(page_pixel_dims(&pdf, 144.0).unwrap(), vec![(200, 200)]);
        let imgs = rasterize(&pdf, 144.0).unwrap();
        assert_eq!((imgs[0].width, imgs[0].height), (200, 200));
        // Junk bytes fail loud, same as rasterize.
        assert!(page_pixel_dims(b"not a pdf", 72.0).is_err());
    }

    #[test]
    fn a_sub_floor_change_still_counts_as_a_difference() {
        // #260: even with an aggressive noise floor, a genuine change smaller than
        // the floor must still make the document read as changed — never a silent
        // "no differences".
        let old = one_square_pdf(10, 10);
        let new = one_square_pdf(60, 60);
        let opts = ImageDiffOptions {
            min_region_px: u32::MAX, // hide everything from the tallies
            ..Default::default()
        };
        let d = diff_pdfs(&old, &new, 72.0, &opts).expect("diff");
        let s = &paired(&d.pages[0]).stats;
        assert_eq!(
            s.added_px, 0,
            "everything below the floor, hidden from tallies"
        );
        assert!(s.suppressed_px > 0, "but the change is surfaced");
        assert!(
            d.any_changes(),
            "a hidden change is still a change (no silent miss)"
        );
    }

    #[test]
    fn overlay_encodes_to_a_valid_png() {
        let d = diff_pdfs(
            &one_square_pdf(10, 10),
            &one_square_pdf(60, 60),
            72.0,
            &ImageDiffOptions::default(),
        )
        .unwrap();
        let png = encode_png(&paired(&d.pages[0]).overlay).expect("encode");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "PNG magic");
        assert!(png.len() > 100);
    }
}
