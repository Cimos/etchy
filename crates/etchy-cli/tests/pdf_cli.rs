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

    /// One synthesized sheet: MediaBox size in points, and its page content
    /// stream (PDF operators).
    struct PageSpec {
        media: (f64, f64),
        content: String,
    }

    /// A 100x100 pt sheet with one filled 20x20 pt black square at (`x`,`y`).
    fn sheet(x: i32, y: i32) -> PageSpec {
        PageSpec {
            media: (100.0, 100.0),
            content: format!("0 0 0 rg\n{x} {y} 20 20 re\nf\n"),
        }
    }

    /// A sheet of an arbitrary MediaBox size with one square at (`x`,`y`).
    fn sized_sheet(w: f64, h: f64, x: i32, y: i32) -> PageSpec {
        PageSpec {
            media: (w, h),
            content: format!("0 0 0 rg\n{x} {y} 20 20 re\nf\n"),
        }
    }

    /// An A4-landscape **line-art** sheet: a thin frame, a title block, a net grid
    /// of hairlines, and `parts` outlined boxes — what a schematic export actually
    /// looks like, and the regime the page digest has to be right in.
    fn line_art_sheet(nets: usize, parts: &[(i32, i32)]) -> PageSpec {
        let mut c = String::from("0 0 0 RG\n0.75 w\n");
        // Frame and inner border.
        c.push_str("8 8 826 579 re\nS\n16 16 810 563 re\nS\n");
        // Title block, bottom right, with divider rows.
        c.push_str("620 20 200 90 re\nS\n");
        for k in 1..5 {
            let y = 20 + k * 18;
            c.push_str(&format!("620 {y} m\n820 {y} l\nS\n"));
        }
        // Net hairlines across the sheet.
        for k in 0..nets {
            let y = 130 + k as i32 * 26;
            c.push_str(&format!("30 {y} m\n810 {y} l\nS\n"));
        }
        for k in 0..nets.min(12) {
            let x = 40 + k as i32 * 62;
            c.push_str(&format!("{x} 130 m\n{x} 560 l\nS\n"));
        }
        // Parts: an outlined box with four pin stubs.
        for (x, y) in parts {
            c.push_str(&format!("{x} {y} 40 28 re\nS\n"));
            for k in 0..4 {
                let py = y + 4 + k * 7;
                c.push_str(&format!("{} {py} m\n{x} {py} l\nS\n", x - 10));
                c.push_str(&format!("{} {py} m\n{} {py} l\nS\n", x + 40, x + 50));
            }
        }
        PageSpec {
            media: (842.0, 595.0),
            content: c,
        }
    }

    /// A well-formed multi-page PDF, one page per [`PageSpec`]. Written to a temp
    /// file and its path returned, so the real binary reads a real file.
    fn write_multi_page_pdf(tag: &str, sheets: &[PageSpec]) -> PathBuf {
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
            let (mw, mh) = s.media;
            let content = &s.content;
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
        let new = write_multi_page_pdf(
            "262-new",
            &[sized_sheet(100.0, 200.0, 10, 10), sheet(60, 60)],
        );
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
    fn one_small_part_added_to_a_line_art_sheet_stays_paired_and_is_located() {
        // #249, the regression that matters end to end: a two-sheet pack where the
        // second sheet gains ONE small part. A page digest that is not local
        // scored that sheet as a different document, unpaired it, and reported
        // "1 sheet inserted + 1 removed" with NO overlay and zero added pixels —
        // strictly worse than index pairing, and a change that is never located.
        let old = write_multi_page_pdf(
            "249-edit-old",
            &[
                line_art_sheet(9, &[(120, 400), (300, 300), (500, 450)]),
                line_art_sheet(7, &[(200, 200), (600, 380)]),
            ],
        );
        let new = write_multi_page_pdf(
            "249-edit-new",
            &[
                line_art_sheet(9, &[(120, 400), (300, 300), (500, 450)]),
                // The same sheet plus one 40x28 pt part in an empty corner.
                line_art_sheet(7, &[(200, 200), (600, 380), (120, 60)]),
            ],
        );
        let out_dir = scratch("249-edit-out");
        let out = etchy()
            .arg(&old)
            .arg(&new)
            .arg("--out")
            .arg(&out_dir)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "an edited sheet is a change: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        // No phantom page-structure change.
        assert!(
            !stdout.contains("new-only") && !stdout.contains("old-only"),
            "an edit must not be reported as an inserted/removed sheet: {stdout}"
        );
        assert!(
            !stdout.contains("inserted") && !stdout.contains("removed at"),
            "nothing was inserted or removed: {stdout}"
        );
        assert!(
            stdout.contains("2 page(s) diffed"),
            "both sheets are pixel-diffed: {stdout}"
        );
        // The change is located, with an overlay to look at.
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
        assert!(json.contains("\"identity\": true"), "{json}");
        assert!(json.contains("\"basis\": \"index\""), "{json}");
        assert!(json.contains("\"paired\": 2"), "{json}");
        // Page 2 carries real added pixels and at least one region.
        let page2 = json.split("\"page\": 2").nth(1).expect("page 2 row");
        let added: u64 = page2
            .split("\"added_px\": ")
            .nth(1)
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse().ok())
            .expect("added_px on page 2");
        assert!(added > 100, "the added part is located: added_px={added}");
        assert!(
            out_dir.join("page-2.png").is_file(),
            "the edited sheet gets an overlay PNG"
        );
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn a_genuine_mid_document_insertion_still_realigns_line_art_sheets() {
        // The other half of #249: a real insertion between line-art sheets must
        // still be found, with the neighbours paired to themselves.
        let a = || line_art_sheet(9, &[(120, 400), (300, 300)]);
        let b = || line_art_sheet(6, &[(500, 200)]);
        let c = || line_art_sheet(12, &[(200, 150), (600, 420), (300, 500)]);
        let x = || line_art_sheet(3, &[(400, 300), (450, 350), (500, 400), (550, 450)]);
        let old = write_multi_page_pdf("249-ins-old", &[a(), b(), c()]);
        let new = write_multi_page_pdf("249-ins-new", &[a(), x(), b(), c()]);
        let out = etchy().arg(&old).arg(&new).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("aligned by page content")
                && stdout.contains("1 sheet inserted at new page 2"),
            "the insertion is found and named: {stdout}"
        );
        assert!(
            stdout.contains("3 sheet(s) paired"),
            "the three original sheets pair with themselves: {stdout}"
        );
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
        assert!(json.contains("\"basis\": \"content\""), "{json}");
        assert_eq!(
            json.matches("\"changed_fraction\": 1.0").count(),
            1,
            "only the inserted sheet is wholly changed: {json}"
        );
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn a_one_pixel_size_rounding_difference_still_produces_a_located_diff() {
        // #262: the SAME A4-landscape sheet with MediaBox [0 0 842 595] against
        // 841.9 rasterizes a pixel narrower at 150 DPI. Comparing dimensions
        // exactly called that a paper-size change and discarded the sheet's whole
        // pixel diff — the moved part was never located.
        let old = write_multi_page_pdf("262-round-old", &[sized_sheet(842.0, 595.0, 40, 40)]);
        let new = write_multi_page_pdf("262-round-new", &[sized_sheet(841.9, 595.0, 300, 300)]);
        let out_dir = scratch("262-round-out");
        let out = etchy()
            .arg(&old)
            .arg(&new)
            .arg("--out")
            .arg(&out_dir)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "a real change on a rounding-sized sheet exits 1: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            !stdout.contains("size change"),
            "sub-pixel rounding is not a paper-size change: {stdout}"
        );
        assert!(
            stdout.contains("page size rounding") && stdout.contains("1753x1239"),
            "the crop is stated, never silent: {stdout}"
        );
        assert!(
            stdout.contains("1 page(s) diffed"),
            "the sheet IS pixel-diffed: {stdout}"
        );
        assert!(
            out_dir.join("page-1.png").is_file(),
            "and it has an overlay to look at"
        );
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
        assert!(json.contains("\"diffed_width\": 1753"), "{json}");
        assert!(!json.contains("size_change"), "{json}");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[test]
    fn a_page_count_over_the_alignment_cap_fails_loud_instead_of_aborting() {
        // Page alignment is a DP matrix quadratic in the page count: two
        // 10 000-page PDFs (a couple of MB each, under every other cap) asked for
        // 800 MB and killed the process with SIGABRT and no message. Over the cap
        // it must be exit 2 naming the input, the count and the limit.
        let pages: Vec<PageSpec> = (0..1025).map(|i| sheet(10 + (i % 40), 10)).collect();
        let big = write_multi_page_pdf("249-cap", &pages);
        let small = write_multi_page_pdf("249-cap-small", &[sheet(10, 10)]);
        for (a, b) in [(&big, &small), (&small, &big)] {
            let out = etchy().arg(a).arg(b).output().unwrap();
            assert_eq!(
                out.status.code(),
                Some(2),
                "over the page cap must exit 2, not abort: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                stderr.contains("1025 pages") && stderr.contains("1024-page limit"),
                "the error names the count and the limit: {stderr}"
            );
            assert!(
                stderr.contains(a.file_name().unwrap().to_str().unwrap())
                    || stderr.contains(b.file_name().unwrap().to_str().unwrap()),
                "the error names the input: {stderr}"
            );
        }
        let _ = std::fs::remove_file(&big);
        let _ = std::fs::remove_file(&small);
    }

    #[test]
    fn a_total_raster_over_the_budget_fails_loud_before_rendering() {
        // Ten 100x100 pt sheets at 3600 DPI are 5000x5000 px = 25 MP each — under
        // the 50 MP per-page cap and far under the page-count cap — but 250 MP a
        // side and 500 MP for the pair, over the 400 MP whole-run budget. Under
        // the per-page cap alone this pair rasterizes ~2 GB and the OS kills the
        // process with no message (#297); it must be exit 2 naming the totals.
        // Neither document breaches the budget on its own: the check has to sum
        // BOTH sides.
        let pages: Vec<PageSpec> = (0..10).map(|i| sheet(10 + i * 5, 10)).collect();
        let old = write_multi_page_pdf("297-budget-old", &pages);
        let new = write_multi_page_pdf("297-budget-new", &pages);
        let out = etchy()
            .arg("--dpi")
            .arg("3600")
            .arg(&old)
            .arg(&new)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(2),
            "over the total raster budget must exit 2, not be OOM-killed: {stderr}"
        );
        assert!(
            stderr.contains("500 MP in total")
                && stderr.contains("old 10 pages ~250 MP")
                && stderr.contains("new 10 pages ~250 MP")
                && stderr.contains("400 MP total raster budget"),
            "the error names the per-document and total pixel counts and the budget: {stderr}"
        );
        assert!(
            stderr.contains("lower --dpi"),
            "the error tells the user to lower --dpi: {stderr}"
        );
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
    }

    #[test]
    fn a_modest_multi_page_pair_under_the_budget_still_diffs() {
        // The same ten-sheet pair at 720 DPI is 1000x1000 px a page: 10 MP a side,
        // 20 MP for the pair — well inside every cap, so it must run to a real
        // result (identical documents: exit 0), not trip the new budget check.
        let pages: Vec<PageSpec> = (0..10).map(|i| sheet(10 + i * 5, 10)).collect();
        let old = write_multi_page_pdf("297-modest-old", &pages);
        let new = write_multi_page_pdf("297-modest-new", &pages);
        let out = etchy()
            .arg("--dpi")
            .arg("720")
            .arg(&old)
            .arg(&new)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(
            out.status.code(),
            Some(0),
            "a modest multi-page pair must diff normally: {stderr}"
        );
        assert!(
            !stderr.contains("raster budget"),
            "under the budget the check must stay silent: {stderr}"
        );
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::remove_file(&new);
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
