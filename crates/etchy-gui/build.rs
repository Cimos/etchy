//! Stamps the build with the git short sha (#213), exposed to the crate as the
//! compile-time env var `ETCHY_BUILD_SHA`. The GUI shows it in the Help menu
//! and the brand icon's tooltip so the running build is provable — including a
//! browser tab that might be serving stale wasm. Falls back to "unknown" when
//! git or the repo isn't available (e.g. building from a source tarball).

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    // Re-stamp when the checked-out commit moves. HEAD alone is NOT enough: a
    // new commit on the same branch leaves HEAD unchanged (it still reads
    // `ref: refs/heads/<branch>`), so cargo would reuse the cached script
    // output and bake a STALE sha into fresh code — defeating the whole point.
    // Watch HEAD, the branch ref HEAD points at, and packed-refs. `git-path`
    // with absolute format resolves all three correctly in both plain checkouts
    // and worktrees.
    for path in ["HEAD", "packed-refs"] {
        if let Some(p) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = git(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &head_ref,
        ]) {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ETCHY_BUILD_SHA={sha}");
    windows_icon();
}

#[cfg(windows)]
fn windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../assets/brand/etchy-app-icon.ico");
        winresource::WindowsResource::new()
            .set_icon("../../assets/brand/etchy-app-icon.ico")
            .compile()
            .expect("embed the Windows app icon");
    }
}

#[cfg(not(windows))]
fn windows_icon() {}
