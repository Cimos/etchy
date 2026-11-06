//! `etchy log <RANGE> [SUBDIR]` — per-commit layer changes over a git range
//! (#424). Walks the first-parent commits in the range that touch the fab pack
//! and diffs each against its first parent, through the same git blob reader,
//! board loader and `compare_detailed` path as the two-ref mode. No checkout.

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use anyhow::{Context, Result};
use clap::Parser;
use etchy_core::report::md_code_span;
use etchy_core::{compare_detailed, Board, BoardBuilder, DiffReport, GerberFormat, LayerStatus};
use serde::Serialize;

use crate::{
    git_bytes, git_stdout, git_subdir, load_board_git, report_with_load_warnings, status_str,
    write_stdout, Exit, Format,
};

/// The JSON contract version of `etchy log --format json`.
const LOG_SCHEMA_VERSION: u32 = 1;

/// List, per commit, which layers changed and by how much over a git range.
///
/// Walks `git rev-list --first-parent <RANGE> -- <SUBDIR>` (newest first) and
/// diffs each commit against its first parent — a merge is therefore reported
/// as everything it brought into the first-parent line. A commit where the fab
/// pack appears or disappears is reported as an added or removed pack. Exit 0
/// when no commit changed any layer, 1 when one did, 2 on any error (including
/// a commit whose files don't parse).
#[derive(Parser, Debug)]
#[command(name = "etchy log", bin_name = "etchy log", version)]
struct LogCli {
    /// A git revision range, as `git rev-list` takes it: `v0.1.0..HEAD`,
    /// `main..feature`, or a single ref for its whole history.
    range: String,
    /// The subdirectory in the repo where the fab files live (default: the repo
    /// root). Relative to the current directory, like a git pathspec.
    subdir: Option<PathBuf>,
    /// Output format: a terminal summary, machine-readable JSON, or GitHub
    /// Markdown (for a CI step-summary / PR comment).
    #[arg(long, value_enum, default_value_t = Format::Summary)]
    format: Format,
    /// Fail (exit 2) when the range holds more than this many commits touching
    /// the fab pack, rather than truncating the history silently.
    #[arg(long, value_name = "N", default_value_t = 200, value_parser = clap::value_parser!(u32).range(1..))]
    max_commits: u32,
}

/// Is the fab pack present on both sides of a commit, or did it appear/vanish?
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PackChange {
    Present,
    Added,
    Removed,
}

impl PackChange {
    fn as_str(self) -> &'static str {
        match self {
            PackChange::Present => "present",
            PackChange::Added => "added",
            PackChange::Removed => "removed",
        }
    }
}

/// One commit's entry: its identity and the diff against its first parent.
#[derive(Serialize)]
struct CommitEntry {
    commit: String,
    /// The first parent, or `null` for a root commit.
    parent: Option<String>,
    subject: String,
    pack: PackChange,
    report: DiffReport,
}

/// The whole `etchy log` result.
#[derive(Serialize)]
struct LogReport {
    schema_version: u32,
    tool_version: &'static str,
    range: String,
    subdir: String,
    /// Always true: commits are walked along, and diffed against, first parents.
    first_parent: bool,
    any_changes: bool,
    /// Newest first, like `git log`.
    commits: Vec<CommitEntry>,
}

type Loaded = (Board, Option<GerberFormat>, Vec<String>);

/// Entry point for `etchy log …`; owns the 0/1/2 exit contract like `main`.
pub(crate) fn main() -> ExitCode {
    // Skip the binary name so clap sees `log` as its own name.
    let cli = LogCli::parse_from(std::env::args_os().skip(1));
    if std::path::Path::new("log").is_dir() {
        eprintln!(
            "etchy: note: `log` is the history subcommand; to diff a directory named log, \
             write ./log"
        );
    }
    match run(&cli) {
        Ok(any_changes) => Exit::from_passed(!any_changes).into(),
        Err(e) => {
            eprintln!("etchy: error: {e:#}");
            Exit::Error.into()
        }
    }
}

fn run(cli: &LogCli) -> Result<bool> {
    // A range starting with `-` would be read by git as an option.
    if cli.range.starts_with('-') {
        anyhow::bail!("<RANGE> `{}` must not start with '-'", cli.range);
    }
    let subdir = git_subdir(cli.subdir.as_deref())?;
    if cli.subdir.is_none() {
        eprintln!(
            "etchy: note: no [SUBDIR] given — diffing every fab file in the whole tree at \
             each commit; pass a subdirectory to scope the comparison"
        );
    }

    let commits = commits_in_range(&cli.range, subdir, cli.max_commits)?;
    let mut entries = Vec::with_capacity(commits.len());
    // Oldest first, so a commit's parent side is usually the previous commit's
    // new side and the board is reused instead of re-read.
    let mut cache: Option<(String, Rc<Loaded>)> = None;
    for sha in commits.iter().rev() {
        let entry = diff_commit(sha, subdir, &mut cache)
            .with_context(|| format!("commit {}", short(sha)))?;
        entries.push(entry);
    }
    entries.reverse();

    let any_changes = entries.iter().any(|e| e.report.any_changes());
    let log = LogReport {
        schema_version: LOG_SCHEMA_VERSION,
        tool_version: env!("CARGO_PKG_VERSION"),
        range: cli.range.clone(),
        subdir: subdir.to_string(),
        first_parent: true,
        any_changes,
        commits: entries,
    };
    let out = match cli.format {
        Format::Json => serde_json::to_string_pretty(&log).expect("LogReport serializes"),
        Format::Md => format_markdown(&log),
        Format::Summary => {
            for e in &log.commits {
                for w in &e.report.warnings {
                    eprintln!("etchy: warning: {}: {w}", short(&e.commit));
                }
            }
            format_summary(&log)
        }
    };
    write_stdout(&out, Exit::from_passed(!any_changes))?;
    Ok(any_changes)
}

/// The first-parent commits in `range` that touch `subdir`, newest first. More
/// than `max` is an error, never a silent truncation.
fn commits_in_range(range: &str, subdir: &str, max: u32) -> Result<Vec<String>> {
    let limit = format!("--max-count={}", u64::from(max) + 1);
    let listing = git_stdout(&["rev-list", "--first-parent", &limit, range, "--", subdir])
        .with_context(|| format!("listing commits in {range} touching {subdir}"))?;
    let commits: Vec<String> = listing.split_whitespace().map(str::to_string).collect();
    if commits.len() > max as usize {
        anyhow::bail!(
            "{range} has more than {max} commits touching {subdir}; narrow the range or \
             raise --max-commits"
        );
    }
    Ok(commits)
}

/// Diff one commit against its first parent.
fn diff_commit(
    sha: &str,
    subdir: &str,
    cache: &mut Option<(String, Rc<Loaded>)>,
) -> Result<CommitEntry> {
    // `git show` without a pathspec: the real parents, not the simplified ones.
    let meta = git_stdout(&["show", "-s", "--format=%P%x00%s", sha])
        .context("reading the commit's parents and subject")?;
    let (parents, subject) = meta.split_once('\0').unwrap_or((meta.as_str(), ""));
    let parent = parents.split_whitespace().next().map(str::to_string);
    let subject = subject.trim_end_matches(['\r', '\n']).to_string();

    let old = match &parent {
        Some(p) => load_side(p, subdir, cache).context("loading the first parent")?,
        None => None,
    };
    let new = load_side(sha, subdir, cache).context("loading the commit")?;
    let pack = match (&old, &new) {
        (None, Some(_)) => PackChange::Added,
        (Some(_), None) => PackChange::Removed,
        _ => PackChange::Present,
    };
    let empty = Rc::new(BoardBuilder::new().finish());
    let old = old.unwrap_or_else(|| empty.clone());
    let new = new.unwrap_or(empty);

    let diff = compare_detailed(&old.0, &new.0).context("comparing with the first parent")?;
    let report = report_with_load_warnings(&diff, old.2.clone(), new.2.clone(), old.1, new.1);
    Ok(CommitEntry {
        commit: sha.to_string(),
        parent,
        subject,
        pack,
        report,
    })
}

/// Load the fab pack at `rev`, or `None` when `subdir` holds no files there.
/// The tree listing (blob ids included) keys a one-entry cache.
fn load_side(
    rev: &str,
    subdir: &str,
    cache: &mut Option<(String, Rc<Loaded>)>,
) -> Result<Option<Rc<Loaded>>> {
    let listing = git_bytes(&["ls-tree", "-r", "-z", rev, "--", subdir])
        .with_context(|| format!("listing files at {}:{subdir}", short(rev)))?;
    if listing.is_empty() {
        return Ok(None);
    }
    let key = String::from_utf8_lossy(&listing).into_owned();
    if let Some((k, board)) = cache.as_ref() {
        if *k == key {
            return Ok(Some(board.clone()));
        }
    }
    let board = Rc::new(load_board_git(rev, subdir)?);
    *cache = Some((key, board.clone()));
    Ok(Some(board))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn changed_layers(report: &DiffReport) -> impl Iterator<Item = &etchy_core::LayerReport> {
    report
        .layers
        .iter()
        .filter(|l| l.status != LayerStatus::Unchanged)
}

fn format_summary(log: &LogReport) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for e in &log.commits {
        let _ = writeln!(s, "{}  {}", short(&e.commit), e.subject);
        match e.pack {
            PackChange::Added => s.push_str("  pack added (no files at the parent)\n"),
            PackChange::Removed => s.push_str("  pack removed (no files at this commit)\n"),
            PackChange::Present => {}
        }
        if !e.report.any_changes() {
            s.push_str("  no layer changes\n\n");
            continue;
        }
        let _ = writeln!(
            s,
            "  {:<16} {:<13} {:>12} {:>12} {:>9} {:>9}",
            "layer", "status", "added_mm2", "removed_mm2", "+regions", "-regions"
        );
        for l in changed_layers(&e.report) {
            let _ = writeln!(
                s,
                "  {:<16} {:<13} {:>12.5} {:>12.5} {:>9} {:>9}",
                l.name(),
                status_str(l.status),
                l.added_area_mm2,
                l.removed_area_mm2,
                l.added_regions,
                l.removed_regions
            );
        }
        s.push('\n');
    }
    let changed = log
        .commits
        .iter()
        .filter(|e| e.report.any_changes())
        .count();
    let _ = writeln!(
        s,
        "{} commit(s) touching {} in {} (first-parent); {} with layer changes",
        log.commits.len(),
        log.subdir,
        log.range,
        changed
    );
    s.push_str(if log.any_changes {
        "result: differences found"
    } else {
        "result: no differences"
    });
    s
}

/// A commit subject or warning as an inert table cell (#298: this text can be
/// posted as a bot PR comment).
fn md_cell(text: &str) -> String {
    md_code_span(text).replace('|', "\\|")
}

fn format_markdown(log: &LogReport) -> String {
    use std::fmt::Write;
    let changed = log
        .commits
        .iter()
        .filter(|e| e.report.any_changes())
        .count();
    let mut s = String::from("## etchy — PCB log\n\n");
    let _ = writeln!(
        s,
        "{} in {} · {} commit(s), {} with layer changes (first-parent, newest first)\n",
        md_code_span(&log.range),
        md_code_span(&log.subdir),
        log.commits.len(),
        changed
    );
    if log.commits.is_empty() {
        s.push_str("✅ **No commits touch the fab pack in this range.**\n");
        return s;
    }
    s.push_str(
        "| commit | subject | pack | layers changed | +mm² | −mm² | +regions | −regions |\n",
    );
    s.push_str("|---|---|---|--:|--:|--:|--:|--:|\n");
    for e in &log.commits {
        let t = &e.report.totals;
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} | {:.4} | {:.4} | {} | {} |",
            short(&e.commit),
            md_cell(&e.subject),
            e.pack.as_str(),
            t.layers_changed,
            t.added_area_mm2,
            t.removed_area_mm2,
            t.added_regions,
            t.removed_regions
        );
    }
    for e in log.commits.iter().filter(|e| e.report.any_changes()) {
        let _ = writeln!(s, "\n### `{}` — changed layers\n", short(&e.commit));
        s.push_str("| layer | status | +mm² | −mm² | +regions | −regions |\n");
        s.push_str("|---|---|--:|--:|--:|--:|\n");
        for l in changed_layers(&e.report) {
            let _ = writeln!(
                s,
                "| {} | {} | {:.4} | {:.4} | {} | {} |",
                l.name(),
                status_str(l.status),
                l.added_area_mm2,
                l.removed_area_mm2,
                l.added_regions,
                l.removed_regions
            );
        }
    }
    let warnings: Vec<_> = log
        .commits
        .iter()
        .flat_map(|e| e.report.warnings.iter().map(move |w| (&e.commit, w)))
        .collect();
    if !warnings.is_empty() {
        s.push_str("\n⚠️ **Warnings**\n");
        for (commit, w) in warnings {
            let _ = writeln!(s, "- `{}`: {}", short(commit), md_code_span(w));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_cli_defaults_and_bounds() {
        let cli = LogCli::try_parse_from(["log", "a..b", "fab"]).unwrap();
        assert_eq!(cli.max_commits, 200);
        assert_eq!(cli.format, Format::Summary);
        assert!(LogCli::try_parse_from(["log", "a..b", "--max-commits", "0"]).is_err());
    }

    #[test]
    fn markdown_subject_is_inert() {
        let log = LogReport {
            schema_version: LOG_SCHEMA_VERSION,
            tool_version: "0",
            range: "a..b".into(),
            subdir: "fab".into(),
            first_parent: true,
            any_changes: false,
            commits: vec![CommitEntry {
                commit: "0123456789abcdef".into(),
                parent: None,
                subject: "@team | [x](y)".into(),
                pack: PackChange::Added,
                report: DiffReport::new(Vec::new(), Vec::new()),
            }],
        };
        let md = format_markdown(&log);
        assert!(
            md.contains("| `0123456789ab` | `@team \\| [x](y)` | added |"),
            "{md}"
        );
    }
}
