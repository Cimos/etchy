//! `etchy log` contract tests (#424): a temp git repo with a few commits of the
//! demo fab pack, driven through the real `etchy` binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn etchy(repo: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_etchy"));
    c.current_dir(repo);
    c
}

fn demo(side: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../etchy-gui/assets/demo")
        .join(side)
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "-c",
            "user.name=etchy",
            "-c",
            "user.email=etchy@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn copy_pack(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        fs::copy(&path, to.join(path.file_name().unwrap())).unwrap();
    }
}

fn commit(repo: &Path, subject: &str, tag: &str) {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", subject]);
    git(repo, &["tag", tag]);
}

/// t1 add pack · t2 notes only · t3 outside the pack · t4 rev B · t5 drop the
/// top silk · t6 remove the pack.
fn history(tag: &str) -> PathBuf {
    let repo = std::env::temp_dir().join(format!("etchy-log-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&repo);
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    let fab = repo.join("fab");
    copy_pack(&demo("old"), &fab);
    commit(&repo, "add the pack", "t1");
    fs::write(fab.join("notes.txt"), "fab notes\n").unwrap();
    commit(&repo, "notes only", "t2");
    fs::write(repo.join("README.md"), "readme\n").unwrap();
    commit(&repo, "outside the pack", "t3");
    copy_pack(&demo("new"), &fab);
    commit(&repo, "rev B", "t4");
    fs::remove_file(fab.join("Mad_RP2040.gto")).unwrap();
    commit(&repo, "drop the top silk", "t5");
    fs::remove_dir_all(&fab).unwrap();
    commit(&repo, "remove the pack", "t6");
    repo
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn log_lists_each_commit_touching_the_pack() {
    let repo = history("walk");
    let out = etchy(&repo)
        .args(["log", "t6", "fab", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "a changed commit exits 1");
    let v = json(&out);
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["any_changes"], true);
    let commits = v["commits"].as_array().unwrap();
    let subjects: Vec<_> = commits
        .iter()
        .map(|c| c["subject"].as_str().unwrap())
        .collect();
    // Newest first; the README commit doesn't touch fab/.
    assert_eq!(
        subjects,
        [
            "remove the pack",
            "drop the top silk",
            "rev B",
            "notes only",
            "add the pack"
        ]
    );
    let packs: Vec<_> = commits
        .iter()
        .map(|c| c["pack"].as_str().unwrap())
        .collect();
    assert_eq!(packs, ["removed", "present", "present", "present", "added"]);
    let changes: Vec<_> = commits
        .iter()
        .map(|c| c["report"]["any_changes"].as_bool().unwrap())
        .collect();
    assert_eq!(changes, [true, true, true, false, true]);
    assert!(commits[4]["parent"].is_null(), "root commit has no parent");
    assert_eq!(commits[0]["parent"], commits[1]["commit"]);

    let silk = commits[1]["report"]["layers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["status"] != "unchanged")
        .collect::<Vec<_>>();
    assert_eq!(silk.len(), 1, "only the top silk changed: {silk:?}");
    assert_eq!(silk[0]["kind"], "top-silk");
    assert_eq!(silk[0]["status"], "removed-layer");
    assert!(silk[0]["removed_area_mm2"].as_f64().unwrap() > 0.0);
    let removed = &commits[0]["report"]["layers"];
    assert!(removed
        .as_array()
        .unwrap()
        .iter()
        .all(|l| l["status"] == "removed-layer"));

    let out = etchy(&repo)
        .args(["log", "t1..t6", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("drop the top silk"), "{text}");
    assert!(text.contains("top-silk"), "{text}");
    assert!(text.contains("removed-layer"), "{text}");
    assert!(text.contains("pack removed"), "{text}");
    assert!(text.contains("4 commit(s) touching fab"), "{text}");
    assert!(text.ends_with("result: differences found\n"), "{text}");

    let out = etchy(&repo)
        .args(["log", "t1..t6", "fab", "--format", "md"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let md = String::from_utf8(out.stdout).unwrap();
    assert!(md.contains("## etchy — PCB log"), "{md}");
    assert!(md.contains("| `drop the top silk` | present | 1 |"), "{md}");
    assert!(md.contains("| top-silk | removed-layer |"), "{md}");
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn log_without_layer_changes_exits_0() {
    let repo = history("unchanged");
    // t1..t3 holds the notes-only commit (and the README one, outside fab/).
    let out = etchy(&repo)
        .args(["log", "t1..t3", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("notes only"), "{text}");
    assert!(text.contains("no layer changes"), "{text}");
    assert!(!text.contains("outside the pack"), "{text}");
    assert!(text.ends_with("result: no differences\n"), "{text}");

    // An empty range is not a difference either.
    let out = etchy(&repo)
        .args(["log", "t3..t3", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn log_over_the_commit_cap_fails_loud() {
    let repo = history("cap");
    let out = etchy(&repo)
        .args(["log", "t6", "fab", "--max-commits", "4"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("more than 4 commits"), "{err}");
    assert!(err.contains("--max-commits"), "{err}");

    let out = etchy(&repo)
        .args(["log", "t6", "fab", "--max-commits", "5"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "exactly at the cap runs");
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn log_names_an_unparseable_commit() {
    let repo = history("broken");
    let fab = repo.join("fab");
    copy_pack(&demo("old"), &fab);
    // Step-and-repeat is unsupported: the load must fail loud, not be skipped.
    fs::write(
        fab.join("Mad_RP2040.gtl"),
        "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.5*%\n%SRX2Y1I5J0*%\nD10*\nX0Y0D03*\nM02*\n",
    )
    .unwrap();
    commit(&repo, "broken copper", "t7");
    let sha = git(&repo, &["rev-parse", "t7"]);
    let out = etchy(&repo)
        .args(["log", "t6..t7", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("commit {}", &sha[..12])), "{err}");

    // A bad range is an error too, and an option-like one never reaches git.
    let out = etchy(&repo)
        .args(["log", "nope..t7", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = etchy(&repo)
        .args(["log", "--", "-n1", "fab"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let _ = fs::remove_dir_all(repo);
}
