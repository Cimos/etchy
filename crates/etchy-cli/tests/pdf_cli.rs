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
