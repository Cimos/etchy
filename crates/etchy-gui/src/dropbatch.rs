//! Web drop batching (#453).
//!
//! eframe's web `drop` handler reads each dropped file in its own async task
//! and pushes it into `RawInput::dropped_files` when that read finishes. egui
//! takes `dropped_files` every frame, so the files of one drop can arrive over
//! several frames. Loading each frame's slice on its own would load a partial
//! board, or put the later files on the other side.
//!
//! [`DropBatch`] collects the files until the whole drop is in. Our own
//! capture-phase drop listener records how many files the browser handed over
//! ([`DropBatch::on_drop`]); the batch is ready once that many have arrived.
//! eframe skips a file whose read fails without telling the app, so a batch
//! that stops growing short of the count fails loud instead of loading a
//! partial set. Files that arrive with no recorded drop (the listener missed
//! it) load once no more have arrived for a short quiet period.
//!
//! The logic is pure (time is passed in) so it is unit-tested on native.

/// With no recorded drop count, files load once none have arrived for this
/// long (seconds).
pub const QUIET_SECS: f64 = 0.3;

/// A drop that stops short of its file count gives up after no new file has
/// arrived for this long (seconds). Reads of local files are fast; this only
/// trips when eframe dropped a file it could not read.
pub const STALL_SECS: f64 = 10.0;

/// What [`DropBatch::poll`] decided this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum DropPoll<F> {
    /// No drop in progress.
    Idle,
    /// A drop is still arriving; poll again on a later frame.
    Waiting,
    /// The whole drop is in: load these files as one set.
    Ready(Vec<F>),
    /// The drop stalled short of its count. Nothing should load.
    Incomplete { got: usize, expected: usize },
}

/// Files of one (or more overlapping) drops, gathered until the set settles.
#[derive(Debug)]
pub struct DropBatch<F> {
    files: Vec<F>,
    /// Files the browser handed over across the pending drop events.
    expected: usize,
    /// Time of the last drop event or file arrival; `None` when idle.
    last_change: Option<f64>,
}

impl<F> Default for DropBatch<F> {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            expected: 0,
            last_change: None,
        }
    }
}

impl<F> DropBatch<F> {
    /// A drop event handed over `count` files. Drops that overlap a pending
    /// one add to its count and load together.
    pub fn on_drop(&mut self, count: usize, now: f64) {
        if count == 0 {
            return;
        }
        self.expected += count;
        self.last_change = Some(now);
    }

    /// Files that finished reading this frame.
    pub fn on_files(&mut self, files: Vec<F>, now: f64) {
        if files.is_empty() {
            return;
        }
        self.files.extend(files);
        self.last_change = Some(now);
    }

    /// Decide whether the pending drop is complete. `Ready` and `Incomplete`
    /// reset the batch.
    pub fn poll(&mut self, now: f64) -> DropPoll<F> {
        let Some(last) = self.last_change else {
            return DropPoll::Idle;
        };
        let idle_for = now - last;
        let got = self.files.len();
        if self.expected > 0 {
            if got >= self.expected {
                return DropPoll::Ready(self.take());
            }
            if idle_for >= STALL_SECS {
                let expected = self.expected;
                self.take();
                return DropPoll::Incomplete { got, expected };
            }
        } else if idle_for >= QUIET_SECS {
            return DropPoll::Ready(self.take());
        }
        DropPoll::Waiting
    }

    fn take(&mut self) -> Vec<F> {
        self.expected = 0;
        self.last_change = None;
        std::mem::take(&mut self.files)
    }
}

/// The error shown when a drop stalls short of its file count.
pub fn incomplete_message(got: usize, expected: usize) -> String {
    format!(
        "only {got} of the {expected} dropped files could be read, so nothing was loaded \
         — try the drop again"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_without_a_drop() {
        let mut b: DropBatch<u32> = DropBatch::default();
        assert_eq!(b.poll(0.0), DropPoll::Idle);
        b.on_drop(0, 0.0);
        b.on_files(vec![], 0.0);
        assert_eq!(b.poll(100.0), DropPoll::Idle);
    }

    #[test]
    fn files_spread_over_frames_load_as_one_set() {
        let mut b = DropBatch::default();
        b.on_drop(20, 0.0);
        assert_eq!(b.poll(0.0), DropPoll::Waiting);
        // The 20 reads finish over several frames.
        let mut t = 0.0;
        for chunk in (0..20u32).collect::<Vec<_>>().chunks(3) {
            t += 0.016;
            b.on_files(chunk.to_vec(), t);
            if chunk.contains(&19) {
                break;
            }
            assert_eq!(b.poll(t), DropPoll::Waiting);
        }
        assert_eq!(b.poll(t), DropPoll::Ready((0..20).collect()));
        assert_eq!(b.poll(t + 1.0), DropPoll::Idle);
    }

    #[test]
    fn all_files_in_one_frame_load_at_once() {
        let mut b = DropBatch::default();
        b.on_drop(2, 1.0);
        b.on_files(vec!["a", "b"], 1.0);
        assert_eq!(b.poll(1.0), DropPoll::Ready(vec!["a", "b"]));
    }

    #[test]
    fn a_slow_read_keeps_waiting_before_the_stall_limit() {
        let mut b = DropBatch::default();
        b.on_drop(2, 0.0);
        b.on_files(vec![1], 0.5);
        assert_eq!(b.poll(0.5 + STALL_SECS - 0.01), DropPoll::Waiting);
        b.on_files(vec![2], 0.5 + STALL_SECS - 0.01);
        assert_eq!(b.poll(0.5 + STALL_SECS), DropPoll::Ready(vec![1, 2]));
    }

    #[test]
    fn a_stalled_drop_fails_loud_and_loads_nothing() {
        let mut b = DropBatch::default();
        b.on_drop(3, 0.0);
        b.on_files(vec![1, 2], 0.1);
        assert_eq!(
            b.poll(0.1 + STALL_SECS),
            DropPoll::Incomplete {
                got: 2,
                expected: 3
            }
        );
        assert_eq!(b.poll(100.0), DropPoll::Idle);
        let msg = incomplete_message(2, 3);
        assert!(msg.contains("2 of the 3"), "{msg}");
    }

    #[test]
    fn a_drop_where_no_file_reads_fails_loud() {
        let mut b: DropBatch<u8> = DropBatch::default();
        b.on_drop(1, 0.0);
        assert_eq!(
            b.poll(STALL_SECS),
            DropPoll::Incomplete {
                got: 0,
                expected: 1
            }
        );
    }

    #[test]
    fn files_without_a_recorded_drop_load_after_a_quiet_period() {
        let mut b = DropBatch::default();
        b.on_files(vec![1], 0.0);
        assert_eq!(b.poll(0.1), DropPoll::Waiting);
        b.on_files(vec![2], 0.2);
        assert_eq!(b.poll(0.2 + QUIET_SECS - 0.01), DropPoll::Waiting);
        assert_eq!(b.poll(0.2 + QUIET_SECS), DropPoll::Ready(vec![1, 2]));
    }

    #[test]
    fn overlapping_drops_load_together() {
        let mut b = DropBatch::default();
        b.on_drop(1, 0.0);
        b.on_drop(1, 0.05);
        b.on_files(vec!["a.zip"], 0.1);
        assert_eq!(b.poll(0.1), DropPoll::Waiting);
        b.on_files(vec!["b.zip"], 0.2);
        assert_eq!(b.poll(0.2), DropPoll::Ready(vec!["a.zip", "b.zip"]));
    }

    #[test]
    fn a_new_drop_after_one_completes_starts_fresh() {
        let mut b = DropBatch::default();
        b.on_drop(1, 0.0);
        b.on_files(vec![1], 0.0);
        assert_eq!(b.poll(0.0), DropPoll::Ready(vec![1]));
        b.on_drop(2, 5.0);
        b.on_files(vec![2], 5.0);
        assert_eq!(b.poll(5.0), DropPoll::Waiting);
        b.on_files(vec![3], 5.1);
        assert_eq!(b.poll(5.1), DropPoll::Ready(vec![2, 3]));
    }
}
