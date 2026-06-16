//! CLI contract tests: the 0/1/2 exit-code contract and `--json` output, driven
//! through the real `etchy` binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
