//! Reload on change (#423): notice when an opened input is re-exported and
//! say when it has settled, so the viewer can re-diff without being reopened.
//!
//! Change detection polls file sizes and modification times; no new
//! dependency and no OS watcher API, so it behaves the same on every desktop
//! platform and on network or FUSE mounts where file events are unreliable.
//! A folder is watched one level deep, matching how the loader reads it.
//!
//! [`Debouncer`] is pure logic over snapshots and a clock, so the quiet-period
//! rule is unit tested without touching the disk or sleeping. [`FileWatcher`]
//! runs it on a background thread and wakes the UI only when a reload is due,
//! so an idle viewer does not repaint just to poll.
//!
//! Native only: the web viewer gets bytes from the browser and has nothing to
//! watch.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// How long the inputs must stay unchanged before a reload fires. A CAD
/// export writes a fab pack file by file over a second or two; reloading in
/// the middle would diff a half-written set.
pub const QUIET: Duration = Duration::from_millis(500);

/// How often the background thread re-reads the file metadata.
pub const POLL: Duration = Duration::from_millis(250);

/// What one path looked like at one moment. `None` means the path did not
/// exist or could not be read, so a deleted or renamed file is a change too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// The state of every watched file, keyed by path. Two snapshots compare
/// equal exactly when no watched file was added, removed, resized or touched.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<PathBuf, Option<Stamp>>);

#[cfg(test)]
impl Snapshot {
    /// Build a snapshot from given entries, for tests.
    pub fn from_entries(entries: impl IntoIterator<Item = (PathBuf, Option<Stamp>)>) -> Self {
        Self(entries.into_iter().collect())
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Read the current state of `roots`. A file root is stamped directly. A
/// folder root contributes its own entry plus one per file directly inside
/// it, so adding, deleting or rewriting a layer file all register. A folder
/// that cannot be listed records `None` for itself, which differs from any
/// listing that worked.
pub fn take(roots: &[PathBuf]) -> Snapshot {
    let mut map = BTreeMap::new();
    for root in roots {
        if root.is_dir() {
            match std::fs::read_dir(root) {
                Ok(entries) => {
                    map.insert(root.clone(), stamp(root));
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            let s = stamp(&p);
                            map.insert(p, s);
                        }
                    }
                }
                Err(_) => {
                    map.insert(root.clone(), None);
                }
            }
        } else {
            map.insert(root.clone(), stamp(root));
        }
    }
    Snapshot(map)
}

/// The quiet-period rule. Fed a snapshot on every poll, it reports a reload
/// once the inputs differ from the last loaded state and have then held still
/// for `quiet`. Every further change during the wait restarts the clock, and
/// a change that is undone before the wait ends is dropped.
#[derive(Debug)]
pub struct Debouncer {
    /// The state the viewer last loaded (or last reloaded for).
    baseline: Snapshot,
    /// A differing state and when it was first seen unchanged.
    pending: Option<(Snapshot, Instant)>,
    quiet: Duration,
}

impl Debouncer {
    pub fn new(baseline: Snapshot, quiet: Duration) -> Self {
        Self {
            baseline,
            pending: None,
            quiet,
        }
    }

    /// Feed the latest snapshot. Returns true when a reload is due; the new
    /// state then becomes the baseline, so one export fires once.
    pub fn observe(&mut self, now: Instant, snap: Snapshot) -> bool {
        if snap == self.baseline {
            self.pending = None;
            return false;
        }
        match &self.pending {
            Some((seen, since)) if *seen == snap => {
                if now.duration_since(*since) >= self.quiet {
                    self.baseline = snap;
                    self.pending = None;
                    return true;
                }
                false
            }
            _ => {
                self.pending = Some((snap, now));
                false
            }
        }
    }
}

/// A background poller over one set of input paths. Dropping it stops the
/// thread at its next poll.
pub struct FileWatcher {
    roots: Vec<PathBuf>,
    stop: Arc<AtomicBool>,
    rx: Receiver<()>,
}

impl FileWatcher {
    /// Start watching `roots`. The baseline is read here, on the caller's
    /// thread, so a change made right after this call is never missed.
    /// `wake` runs on the watcher thread whenever a reload becomes due.
    pub fn spawn(roots: Vec<PathBuf>, wake: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<()>, Receiver<()>) = std::sync::mpsc::channel();
        let baseline = take(&roots);
        let thread_roots = roots.clone();
        let thread_stop = Arc::clone(&stop);
        let spawned = std::thread::Builder::new()
            .name("etchy-watch".into())
            .spawn(move || {
                let mut deb = Debouncer::new(baseline, QUIET);
                while !thread_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL);
                    if thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if deb.observe(Instant::now(), take(&thread_roots)) {
                        if tx.send(()).is_err() {
                            break;
                        }
                        wake();
                    }
                }
            });
        // A thread that cannot start leaves a watcher that never fires; the
        // viewer still works, it just will not auto-reload.
        if let Err(e) = spawned {
            eprintln!("etchy-gui: could not start the file watcher: {e}");
        }
        Self { roots, stop, rx }
    }

    /// The paths this watcher covers.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// True if at least one reload became due since the last call. Several
    /// queued signals collapse into one reload.
    pub fn take_change(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(len: u64) -> Snapshot {
        Snapshot::from_entries([(
            PathBuf::from("a.gbr"),
            Some(Stamp {
                len,
                modified: None,
            }),
        )])
    }

    const Q: Duration = Duration::from_millis(500);

    #[test]
    fn unchanged_inputs_never_fire() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(snap(1), Q);
        for ms in [0, 300, 600, 5000] {
            assert!(!d.observe(t0 + Duration::from_millis(ms), snap(1)));
        }
    }

    #[test]
    fn a_change_fires_once_after_the_quiet_period() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(snap(1), Q);
        assert!(!d.observe(t0, snap(2)), "first sight only starts the clock");
        assert!(!d.observe(t0 + Duration::from_millis(250), snap(2)));
        assert!(d.observe(t0 + Duration::from_millis(500), snap(2)));
        // The settled state is the new baseline: no repeat fire.
        assert!(!d.observe(t0 + Duration::from_millis(750), snap(2)));
        assert!(!d.observe(t0 + Duration::from_millis(5000), snap(2)));
    }

    #[test]
    fn writes_during_the_wait_restart_the_clock() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(snap(1), Q);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(!d.observe(at(0), snap(2)));
        assert!(!d.observe(at(400), snap(3))); // still being written
        assert!(!d.observe(at(800), snap(3)), "only 400 ms quiet so far");
        assert!(d.observe(at(900), snap(3)));
    }

    #[test]
    fn a_change_undone_before_the_wait_ends_is_dropped() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(snap(1), Q);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(!d.observe(at(0), snap(2)));
        assert!(!d.observe(at(250), snap(1)));
        assert!(!d.observe(at(1000), snap(1)));
    }

    #[test]
    fn a_second_export_fires_again() {
        let t0 = Instant::now();
        let mut d = Debouncer::new(snap(1), Q);
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(!d.observe(at(0), snap(2)));
        assert!(d.observe(at(500), snap(2)));
        assert!(!d.observe(at(2000), snap(3)));
        assert!(d.observe(at(2500), snap(3)));
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("etchy-watch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folder_snapshot_sees_added_rewritten_and_removed_files() {
        let dir = scratch("folder");
        let roots = vec![dir.clone()];
        std::fs::write(dir.join("top.gtl"), b"G04*").unwrap();
        let s0 = take(&roots);
        assert_eq!(take(&roots), s0, "nothing changed");

        std::fs::write(dir.join("top.gtl"), b"G04 longer*").unwrap();
        let s1 = take(&roots);
        assert_ne!(s1, s0, "a resized file is a change");

        std::fs::write(dir.join("bot.gbl"), b"G04*").unwrap();
        let s2 = take(&roots);
        assert_ne!(s2, s1, "an added file is a change");

        std::fs::remove_file(dir.join("bot.gbl")).unwrap();
        assert_ne!(take(&roots), s2, "a removed file is a change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_snapshot_sees_a_rewrite_and_a_delete() {
        let dir = scratch("file");
        let f = dir.join("pack.zip");
        std::fs::write(&f, b"one").unwrap();
        let roots = vec![f.clone()];
        let s0 = take(&roots);
        std::fs::write(&f, b"three").unwrap();
        let s1 = take(&roots);
        assert_ne!(s1, s0);
        std::fs::remove_file(&f).unwrap();
        let s2 = take(&roots);
        assert_ne!(s2, s1);
        assert_eq!(s2, Snapshot::from_entries([(f, None)]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn watcher_signals_after_a_settled_change() {
        let dir = scratch("thread");
        let f = dir.join("top.gtl");
        std::fs::write(&f, b"one").unwrap();
        let woke = Arc::new(AtomicBool::new(false));
        let w = Arc::clone(&woke);
        let watcher = FileWatcher::spawn(vec![dir.clone()], move || {
            w.store(true, Ordering::Relaxed);
        });
        assert_eq!(watcher.roots(), std::slice::from_ref(&dir));
        std::fs::write(&f, b"a longer body").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut fired = false;
        while Instant::now() < deadline {
            if watcher.take_change() {
                fired = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(fired, "the watcher should report the rewrite");
        assert!(woke.load(Ordering::Relaxed), "and wake the UI");
        assert!(!watcher.take_change(), "one change, one signal");
        drop(watcher);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
