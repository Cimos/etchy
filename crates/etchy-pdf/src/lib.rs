//! etchy-pdf — schematic-PDF page-by-page pixel diff. Phase-0 stub.
//!
//! Will wrap `pdfium-render` (permissive; static-linked) to rasterize pages,
//! then reuse etchy-core's image-diff. Implementation in Milestone 3 — see
//! docs/DEVELOPER_GUIDE.md (PDF raster). Watch `hayro` (pure-Rust) to one day
//! drop the C++ dependency.

/// Whether the PDF backend is compiled into this build.
pub fn available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn stub_unavailable() {
        assert!(!super::available());
    }
}
