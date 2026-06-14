//! etchy — fast, trustworthy PCB visual + geometric diff (CLI; primary surface).
//!
//! Phase-0 scaffold: the exit-code contract + wiring. Real arg parsing (clap)
//! and the diff pipeline land in Milestone 1 (see docs/ROADMAP.md).

use std::process::ExitCode;

/// etchy's CI exit-code contract. Defined up front; variants are wired in as the
/// pipeline lands.
#[allow(dead_code)]
#[repr(i32)]
enum Exit {
    /// No differences found.
    NoDiff = 0,
    /// Differences found (used by `--fail-on-diff` / CI gating).
    DiffFound = 1,
    /// An error prevented a comparison.
    Error = 2,
}

impl From<Exit> for ExitCode {
    fn from(code: Exit) -> Self {
        ExitCode::from(code as u8)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("etchy {}", etchy_core::version());
        return Exit::NoDiff.into();
    }
    eprintln!(
        "etchy {} — PCB visual + geometric diff\n\
         usage: etchy <old> <new>   (not yet implemented — Phase-0 scaffold)\n\
         roadmap: docs/ROADMAP.md",
        etchy_core::version()
    );
    Exit::Error.into()
}
