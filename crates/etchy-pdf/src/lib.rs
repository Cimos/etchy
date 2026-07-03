//! etchy-pdf — schematic-PDF page-by-page pixel diff.
//!
//! Feature-gated behind `pdf` so the pure-Rust PDF stack (hayro + friends) and its
//! higher MSRV stay out of the default geometric-diff binaries. With the feature
//! off, [`available`] returns `false` and nothing heavy is compiled.
//!
//! With `--features pdf`: [`rasterize`] renders each page of a PDF to an RGBA
//! [`etchy_core::Image`] via hayro (no C++/PDFium), and [`diff_pdfs`] pairs the two
//! revisions' pages by index and runs `etchy_core`'s raster diff on each. Page
//! pairing is by index — etchy diffs same-document revisions and does not realign
//! inserted or deleted sheets (it reports the page-count change instead); a page
//! whose size changed fails loud, mirroring the geometry same-board guard.

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
    use etchy_core::{diff_images, EngineError, Image, ImageDiffOptions, ImageDiffResult};
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
        /// A page-pair diff failed (e.g. a page changed size between revisions).
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

    /// One page-pair's diff, tagged with its 1-based page number.
    pub struct PageDiff {
        pub page: usize,
        pub diff: ImageDiffResult,
    }

    /// The whole-document diff: per-paired-page results plus each side's page count
    /// (they can differ — an added/removed sheet — which the caller surfaces).
    pub struct PdfDiff {
        pub old_pages: usize,
        pub new_pages: usize,
        pub pages: Vec<PageDiff>,
    }

    impl PdfDiff {
        /// Any pixel change on any paired page, or a page-count change.
        pub fn any_changes(&self) -> bool {
            self.old_pages != self.new_pages
                || self
                    .pages
                    .iter()
                    .any(|p| p.diff.stats.changed_fraction > 0.0)
        }
    }

    /// Rasterize both PDFs at `dpi` and diff their pages pairwise by index. Extra
    /// pages on either side are reported via the page counts, not diffed. A paired
    /// page whose size changed between revisions fails loud (`ImageSizeMismatch`).
    pub fn diff_pdfs(
        old: &[u8],
        new: &[u8],
        dpi: f32,
        opts: &ImageDiffOptions,
    ) -> Result<PdfDiff, PdfError> {
        let old_imgs = rasterize(old, dpi)?;
        let new_imgs = rasterize(new, dpi)?;
        let paired = old_imgs.len().min(new_imgs.len());
        let mut pages = Vec::with_capacity(paired);
        for i in 0..paired {
            let diff = diff_images(&old_imgs[i], &new_imgs[i], opts)?;
            pages.push(PageDiff { page: i + 1, diff });
        }
        Ok(PdfDiff {
            old_pages: old_imgs.len(),
            new_pages: new_imgs.len(),
            pages,
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
pub use imp::{diff_pdfs, encode_png, rasterize, PageDiff, PdfDiff, PdfError, DEFAULT_DPI};

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
        assert_eq!(d.pages[0].diff.stats.changed_fraction, 0.0);
    }

    #[test]
    fn a_moved_square_is_detected_as_added_and_removed() {
        // Same square, different position → old location removed, new location added.
        let old = one_square_pdf(10, 10);
        let new = one_square_pdf(60, 60);
        let d = diff_pdfs(&old, &new, 72.0, &ImageDiffOptions::default()).expect("diff");
        assert!(d.any_changes());
        let s = &d.pages[0].diff.stats;
        assert!(s.added_px > 100, "new square painted, got {}", s.added_px);
        assert!(s.removed_px > 100, "old square gone, got {}", s.removed_px);
        assert!(s.regions >= 2, "two disjoint squares, got {}", s.regions);
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
        let png = encode_png(&d.pages[0].diff.overlay).expect("encode");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "PNG magic");
        assert!(png.len() > 100);
    }
}
