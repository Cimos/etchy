//! CLI contract tests for the PDF pixel-diff path (CLI-6), driven through the
//! real `etchy` binary on the committed KiCad-demo fixture pair
//! (`corpus/pdf/`, see its README for provenance).

use std::path::PathBuf;
use std::process::Command;

fn etchy() -> Command {
    Command::new(env!("CARGO_BIN_EXE_etchy"))
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/pdf")
        .join(name)
}

// The PDF/non-PDF mismatch check runs before any PDF decoding, so it is
// feature-independent — this test guards it in the default build too (#263).
#[test]
fn missing_second_input_is_not_labelled_not_a_pdf() {
    // `etchy real.pdf missing.pdf`: the old code called the missing file "is not
    // a PDF", conflating "absent" with "wrong type". It must now say the path
    // does not exist. (Exit code was already correct at 2.)
    let missing =
        std::env::temp_dir().join(format!("etchy-263-missing-{}.pdf", std::process::id()));
    let _ = std::fs::remove_file(&missing);
    let out = etchy()
        .arg(fixture("old.pdf"))
        .arg(&missing)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "a mismatch still exits 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not exist"),
        "the missing file is named as missing, not 'not a PDF': {stderr}"
    );
    assert!(
        !stderr.contains("is not a PDF"),
        "must not mislabel a missing path as wrong-type: {stderr}"
    );
}

#[cfg(feature = "pdf")]
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etchy-pdf-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(feature = "pdf")]
mod with_pdf {
    use super::*;

    #[test]
    fn changed_pdf_exits_1_and_writes_overlays() {
        let out_dir = scratch("out");
        let out = etchy()
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .arg("--out")
            .arg(&out_dir)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "a changed PDF exits 1");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("differences found"), "summary: {stdout}");
        // The summary carries the per-page table; the fixture's edit is on page 1.
        assert!(stdout.lines().any(|l| l.trim_start().starts_with('1')));
        let png = out_dir.join("page-1.png");
        assert!(png.is_file(), "--out writes page-1.png");
        assert!(
            std::fs::read(&png)
                .unwrap()
                .starts_with(&[0x89, b'P', b'N', b'G']),
            "page-1.png is a PNG"
        );
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn identical_pdfs_exit_0() {
        let out = etchy()
            .arg(fixture("old.pdf"))
            .arg(fixture("old.pdf"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "a PDF against itself exits 0");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("no differences"), "summary: {stdout}");
    }

    #[test]
    fn json_carries_the_pdf_schema() {
        let out = etchy()
            .arg("--format")
            .arg("json")
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        let json = String::from_utf8_lossy(&out.stdout);
        assert!(json.contains("\"schema_version\": 1"), "json: {json}");
        assert!(json.contains("\"any_changes\": true"));
        assert!(json.contains("\"present\": \"both\""));
    }

    #[test]
    fn one_pdf_one_directory_is_a_loud_error() {
        let dir = scratch("mixed");
        let out = etchy().arg(fixture("old.pdf")).arg(&dir).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "mixed inputs exit 2");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("both inputs must be PDFs"),
            "stderr: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copper_gates_with_pdf_inputs_are_a_loud_error() {
        for args in [
            &["--fail-on-area", "0.5"][..],
            &["--fail-on-regions", "3"][..],
            &["--gate-layers", "copper"][..],
        ] {
            let out = etchy()
                .args(args)
                .arg(fixture("old.pdf"))
                .arg(fixture("new.pdf"))
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(2), "{args:?} must exit 2");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(stderr.contains("not valid for PDF"), "stderr: {stderr}");
        }
    }

    #[test]
    fn svg_and_html_with_pdf_inputs_are_a_loud_error() {
        let dir = scratch("svg");
        let out = etchy()
            .arg("--svg")
            .arg(&dir)
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("not valid for PDF"), "stderr: {stderr}");
        assert!(
            stderr.contains("--out"),
            "the error points at --out: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn min_region_px_floor_is_surfaced_and_still_counts_as_a_change() {
        // A huge floor hides every changed region from the overlay/tallies, but the
        // change must still exit 1 and the summary must say what was hidden — the
        // noise floor never turns a real change into "no differences" (#260).
        let out = etchy()
            .arg("--min-region-px")
            .arg("100000000")
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "a change hidden by the floor still exits 1"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("differences found"), "summary: {stdout}");
        assert!(
            stdout.contains("noise floor") && stdout.contains("min-region-px"),
            "the hidden change is surfaced: {stdout}"
        );
    }

    #[test]
    fn min_region_px_on_the_gerber_path_is_a_loud_error() {
        // A PDF-only flag on the geometry path must fail loud, not silently no-op.
        let a = scratch("gerber-a");
        let b = scratch("gerber-b");
        let out = etchy()
            .arg("--min-region-px")
            .arg("4")
            .arg(&a)
            .arg(&b)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("--min-region-px") && stderr.contains("PDF"),
            "stderr points at the PDF-only flag: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn oversized_dpi_hits_the_pixel_cap_loudly() {
        // An A4-ish sheet at 20000 DPI is far over the ~50 MP per-page cap.
        let out = etchy()
            .arg("--dpi")
            .arg("20000")
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "over-cap exits 2 before rendering"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("--dpi"),
            "the error mentions --dpi: {stderr}"
        );
    }

    /// A well-formed multi-page PDF: one page per `(media_w, media_h, sq_x, sq_y)`
    /// sheet, each carrying one filled 20x20 pt black square. Written to a temp
    /// file and its path returned, so the real binary reads a real file.
    fn write_multi_page_pdf(tag: &str, sheets: &[(i32, i32, i32, i32)]) -> PathBuf {
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
        for (i, (mw, mh, x, y)) in sheets.iter().enumerate() {
            let content = format!("0 0 0 rg\n{x} {y} 20 20 re\nf\n");
            objs.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {mw} {mh}] \
                 /Contents {} 0 R /Resources << >> >>",
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
        let path = std::env::temp_dir().join(format!("etchy-pdf-{}-{tag}.pdf", std::process::id()));
        std::fs::write(&path, pdf).unwrap();
        path
    }

    /// A 100x100 pt sheet with its square at (`x`,`y`).
    fn sheet(x: i32, y: i32) -> (i32, i32, i32, i32) {
        (100, 100, x, y)
    }

    #[test]
    fn a_mid_document_insertion_is_realigned_and_reported() {
        // #249: old [A, B, C]; new [A, X, B, C]. Index pairing reported every
        // later sheet as heavily changed; content alignment must pair them and
        // name the inserted sheet in the output.
        let old = write_multi_page_pdf("249-old", &[sheet(10, 10), sheet(60, 60), sheet(10, 60)]);
        let new = write_multi_page_pdf(
            "249-new",
            &[sheet(10, 10), sheet(60, 10), sheet(60, 60), sheet(10, 60)],
        );
        let out = etchy().arg(&old).arg(&new).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "an inserted sheet exits 1");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("aligned by page content")
                && stdout.contains("1 sheet inserted at new page 2"),
            "the alignment is reported: {stdout}"
        );
        assert!(
            stdout.contains("3 sheet(s) paired"),
            "the three original sheets still pair: {stdout}"
        );
        assert!(stdout.contains("new-only"), "the insert is a row: {stdout}");
        // Exactly one row is a change: the inserted sheet.
        let json = String::from_utf8_lossy(
            &etchy()
                .arg("--format")
                .arg("json")
                .arg(&old)
                .arg(&new)
                .output()
                .unwrap()
                .stdout,
        )
        .to_string();
        let changed = json.matches("\"changed_fraction\": 1.0").count();
        assert_eq!(
            changed, 1,
            "only the inserted sheet is wholly changed: {json}"
        );
        assert!(
            json.contains("\"inserted_new_pages\": [\n      2\n    ]"),
            "{json}"
        );
        assert!(json.contains("\"paired\": 3"), "{json}");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn a_page_size_change_exits_1_with_a_size_note_not_exit_2() {
        // #262: a sheet resized between revisions is a legitimate revision diff.
        // It must exit 1 (differences found) with the size change spelled out —
        // exit 2 is reserved for input etchy cannot render at all, and a CI gate
        // treats it as infrastructure failure rather than a review block.
        let old = write_multi_page_pdf("262-old", &[sheet(10, 10), sheet(60, 60)]);
        let new = write_multi_page_pdf("262-new", &[(100, 200, 10, 10), sheet(60, 60)]);
        let out_dir = scratch("262-out");
        let out = etchy()
            .arg("--dpi")
            .arg("72")
            .arg(&old)
            .arg(&new)
            .arg("--out")
            .arg(&out_dir)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "a resized sheet is a diff, not an error: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("differences found"), "{stdout}");
        assert!(
            stdout.contains("size change") && stdout.contains("100x100 px -> 100x200 px"),
            "the summary names both sizes: {stdout}"
        );
        assert!(
            stdout.contains("1 pixel-diffed, 1 resized"),
            "the count line does not claim the resized sheet was diffed: {stdout}"
        );
        // The rest of the document still diffed, and the unaffected page 2 is
        // clean (no overlay written for it, nothing changed).
        assert!(
            !out_dir.join("page-1.png").exists(),
            "a resized sheet has no overlay to write"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("no overlay for page 1") && stderr.contains("page size changed"),
            "the missing overlay is explained: {stderr}"
        );
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn zero_pixel_dpi_fails_loud_not_no_change() {
        // A tiny positive DPI floors the page to 0x0 px. Diffing zero pixels
        // would read as "no differences" — a silent false negative — so it must
        // fail loud (exit 2), never exit 0.
        let out = etchy()
            .arg("--dpi")
            .arg("0.05")
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "a 0x0-px page must not report no-change"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("raise --dpi"),
            "the error tells the user to raise --dpi: {stderr}"
        );
    }
}

#[cfg(not(feature = "pdf"))]
mod without_pdf {
    use super::*;

    #[test]
    fn pdf_input_without_the_feature_is_a_loud_actionable_error() {
        let out = etchy()
            .arg(fixture("old.pdf"))
            .arg(fixture("new.pdf"))
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "no silent skip");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("rebuild with --features pdf"),
            "stderr: {stderr}"
        );
    }
}
