//! Stamps the build with the git short sha (#213), exposed to the crate as the
//! compile-time env var `ETCHY_BUILD_SHA`. The GUI shows it in the Help menu
//! and the brand icon's tooltip so the running build is provable — including a
//! browser tab that might be serving stale wasm. Falls back to "unknown" when
//! git or the repo isn't available (e.g. building from a source tarball).

use std::process::Command;

fn main() {
    // Re-stamp when the checked-out commit moves. In a plain checkout this is
    // the HEAD file; in a worktree `.git` is a file and the path doesn't exist,
    // which just makes cargo re-run this (cheap) script each build instead.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ETCHY_BUILD_SHA={sha}");
}
