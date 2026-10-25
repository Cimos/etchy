//! CLI contract tests: the 0/1/2 exit-code contract and `--json` output, driven
//! through the real `etchy` binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HDR: &str = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\nD10*\n";
// Corner pads fix the board extent so the same-board guard passes in both revs.
const CORNERS: &str = "X5000000Y5000000D03*\nX50000000Y50000000D03*\n";

fn etchy() -> Command {
    Command::new(env!("CARGO_BIN_EXE_etchy"))
}

/// Create a unique scratch dir under the target tmp area.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("etchy-cli-{}-{}", std::process::id(), tag));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_layer(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), format!("{HDR}{CORNERS}{body}M02*\n")).unwrap();
}

#[test]
fn exit_codes_and_json_contract() {
    let root = scratch("contract");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();

    // old: just the corner pads. new: corners + an added pad inside the extent.
    write_layer(&old, "F_Cu.gbr", "");
    write_layer(&new, "F_Cu.gbr", "X25000000Y25000000D03*\n");

    // diff found -> exit 1
    let out = etchy().arg(&old).arg(&new).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "diff should exit 1");
    assert!(String::from_utf8_lossy(&out.stdout).contains("differences found"));

    // identical dirs -> exit 0
    let out = etchy().arg(&old).arg(&old).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "no diff should exit 0");

    // --json -> parseable contract with schema_version 1
    let out = etchy().arg("--json").arg(&old).arg(&new).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let json = String::from_utf8_lossy(&out.stdout);
    assert!(json.contains("\"schema_version\": 1"), "json: {json}");
    assert!(json.contains("\"added_regions\": 1"));

    // missing directory -> error exit 2
    let out = etchy().arg(root.join("nope")).arg(&new).output().unwrap();
    assert_eq!(out.status.code(), Some(2), "missing dir should exit 2");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn x2_region_labels_reach_json_markdown_and_html() {
    let root = scratch("x2-regions");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    write_layer(&old, "F_Cu.gbr", "");
    write_layer(
        &new,
        "F_Cu.gbr",
        "%TO.P,U3,7*%\n%TO.N,GND*%\nX25000000Y25000000D03*\n",
    );
    let out = etchy()
        .arg("--format")
        .arg("json")
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let layer = &value["layers"][0];
    assert_eq!(layer["labels_available"], true);
    assert_eq!(layer["regions"][0]["change"], "added");
    assert_eq!(layer["regions"][0]["labels"]["refdes"][0], "U3");
    assert_eq!(layer["regions"][0]["labels"]["pins"][0], "U3:7");
    assert_eq!(layer["regions"][0]["labels"]["nets"][0], "GND");

    let html = root.join("report.html");
    let out = etchy()
        .arg("--format")
        .arg("md")
        .arg("--top-regions")
        .arg("1")
        .arg("--html")
        .arg(&html)
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let md = String::from_utf8(out.stdout).unwrap();
    assert!(md.contains("largest changed regions (top 1)"));
    assert!(md.contains("U3:7"));
    let html_text = fs::read_to_string(html).unwrap();
    assert!(html_text.contains("Largest changed regions (top 1)"));
    assert!(html_text.contains("U3:7"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn drill_table_reaches_json_markdown_html_and_summary() {
    let root = scratch("drill-table");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    // Old is metric; new is inch, adds one 0.8 mm (0.0315 in) hole and a
    // 1.0 mm slot. The 0.3 mm holes at the corners keep the board extent.
    fs::write(
        old.join("board-PTH.drl"),
        "M48\nMETRIC,TZ\nT1C0.300\nT2C0.800\n%\nT1\nX5.0Y5.0\nX50.0Y50.0\nT2\nX20.0Y20.0\nM30\n",
    )
    .unwrap();
    fs::write(
        new.join("board-PTH.drl"),
        "M48\nINCH,TZ\nT1C0.0118\nT2C0.0315\nT3C0.0394\n%\nT1\nX0.19685Y0.19685\nX1.9685Y1.9685\n\
         T2\nX0.7874Y0.7874\nX1.1811Y0.7874\nT3\nX0.7874Y1.1811G85X1.1811Y1.1811\nM30\n",
    )
    .unwrap();
    let html = root.join("report.html");
    let run = |format: &str| {
        let out = etchy()
            .arg("--format")
            .arg(format)
            .arg("--html")
            .arg(&html)
            .arg(&old)
            .arg(&new)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
        String::from_utf8(out.stdout).unwrap()
    };

    let json = run("json");
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let table = &value["layers"][0]["drill_table"];
    let rows = table["rows"].as_array().unwrap();
    let summary: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r["diameter_um"].as_i64().unwrap(),
                r["change"].as_str().unwrap(),
                r["old_holes"].as_u64().unwrap(),
                r["new_holes"].as_u64().unwrap(),
                r["new_slots"].as_u64().unwrap(),
            )
        })
        .collect();
    // 0.0118 in = 299.7 µm rounds to 300 and pairs with the metric 0.3 mm tool.
    assert_eq!(
        summary,
        [
            (300, "unchanged", 2, 2, 0),
            (800, "changed", 1, 2, 0),
            (1001, "added", 0, 0, 1),
        ]
    );
    assert_eq!(table["total"]["new_holes"], 4);
    assert_eq!(json, run("json"), "JSON identical across runs");

    let md = run("md");
    assert!(md.contains("### drill-pth — drill table"), "{md}");
    assert!(md.contains("| 0.800 | 1 | 2 | 0 | 0 | changed |"), "{md}");
    let html_text = fs::read_to_string(&html).unwrap();
    assert!(html_text.contains("<h3>Drill table</h3>"));

    let text = run("summary");
    assert!(
        text.contains("drill-pth 0.800 mm: 1 -> 2 holes, 0 -> 0 slots (changed)"),
        "{text}"
    );
    assert!(text.contains("drill-pth total: 3 -> 4 holes, 0 -> 1 slots"));
    assert!(
        !text.contains("drill-pth 0.300"),
        "unchanged sizes stay out of the summary"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn passing_gate_output_matches_exit_0() {
    // #258: a change smaller than --fail-on-area exits 0, but the summary/JSON
    // used to say "differences found" / any_changes:true with no hint that the
    // gate passed. Output must now reconcile with the exit code.
    let root = scratch("gate258");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    write_layer(&old, "F_Cu.gbr", "");
    // A single tiny pad — well under a 5.0 mm² gate.
    write_layer(&new, "F_Cu.gbr", "X25000000Y25000000D03*\n");

    // Summary: exit 0, and the result line explains the passing gate.
    let out = etchy()
        .arg("--fail-on-area")
        .arg("5.0")
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "within-gate change exits 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("gate PASS") && stdout.contains("exit 0"),
        "summary reconciles with exit 0: {stdout}"
    );

    // JSON: the gate verdict is attached and matches the exit code.
    let out = etchy()
        .arg("--format")
        .arg("json")
        .arg("--fail-on-area")
        .arg("5.0")
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let json = String::from_utf8_lossy(&out.stdout);
    assert!(json.contains("\"gate\""), "json carries the gate: {json}");
    assert!(json.contains("\"passed\": true"));
    assert!(json.contains("\"configured\": true"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn nan_area_threshold_is_a_loud_exit_2() {
    // #295: `--fail-on-area nan` used to parse fine and disarm the gate (every
    // `area > NaN` is false, so a changed pair exited 0 with `gate PASS`). A
    // non-finite or negative threshold must be refused before the diff runs.
    let root = scratch("gate295");
    let old = root.join("old");
    fs::create_dir_all(&old).unwrap();
    write_layer(&old, "F_Cu.gbr", "");

    let out = etchy()
        .arg("--fail-on-area")
        .arg("nan")
        .arg(&old)
        .arg(&old)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "nan threshold must exit 2");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--fail-on-area"),
        "stderr names the flag: {err}"
    );
    assert!(err.contains("NaN"), "stderr names the value: {err}");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn closed_stdout_pipe_does_not_panic() {
    // #261: `etchy … | head` closes the read end early. `println!` panics on the
    // resulting broken pipe and the process exits 101, outside the 0/1/2
    // contract. Reproduce by spawning with a piped stdout and dropping the read
    // end before etchy writes: the diff runs, then the first write finds the pipe
    // closed. The fix must exit cleanly, never 101 — and with the verdict the run
    // reached, not an unconditional 0 (#296): losing the reader loses the text,
    // not the 0/1 gate result.
    let root = scratch("brokenpipe");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();
    write_layer(&old, "F_Cu.gbr", "");
    write_layer(&new, "F_Cu.gbr", "X25000000Y25000000D03*\n");

    // (new revision, expected exit): a changed pair is 1, an identical pair 0.
    for (rhs, expected) in [(&new, 1), (&old, 0)] {
        for format in ["summary", "json", "md"] {
            let mut child = etchy()
                .arg("--format")
                .arg(format)
                .arg(&old)
                .arg(rhs)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            // Close the only reader of the pipe immediately, without reading a byte.
            drop(child.stdout.take());
            let status = child.wait().unwrap();
            assert_ne!(
                status.code(),
                Some(101),
                "a closed pipe must not panic (--format {format})"
            );
            assert_eq!(
                status.code(),
                Some(expected),
                "a closed pipe keeps the run's verdict (--format {format})"
            );
        }
    }

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn separator_only_gate_layers_is_an_error() {
    let root = scratch("gate-layers-separators");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();

    // A real change on copper. `--gate-layers ,` used to parse to a filter that
    // matched no layer, so this pair passed the gate and exited 0 (#294). It
    // must be a loud exit 2 before the diff even runs.
    write_layer(&old, "F_Cu.gbr", "");
    write_layer(&new, "F_Cu.gbr", "X25000000Y25000000D03*\n");

    let out = etchy()
        .arg("--gate-layers")
        .arg(",")
        .arg(&old)
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "separator-only --gate-layers should exit 2"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--gate-layers has no groups"),
        "stderr: {stderr}"
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn unsupported_geometry_fails_loud() {
    let root = scratch("unsupported");
    let old = root.join("old");
    let new = root.join("new");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir_all(&new).unwrap();

    // Step-and-repeat (%SR) is geometry-affecting and not yet supported -> must
    // fail loud (exit 2), never a silent or wrong-but-quiet diff.
    write_layer(&old, "F_Cu.gbr", "");
    fs::write(
        new.join("F_Cu.gbr"),
        format!("{HDR}{CORNERS}%SRX2Y1I5J0*%\nX10000000Y10000000D03*\nM02*\n"),
    )
    .unwrap();

    let out = etchy().arg(&old).arg(&new).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "unsupported geometry should exit 2"
    );
    assert!(String::from_utf8_lossy(&out.stderr)
        .to_lowercase()
        .contains("unsupported"));

    let _ = fs::remove_dir_all(&root);
}
