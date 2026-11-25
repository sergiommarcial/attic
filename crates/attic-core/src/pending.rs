//! Folders waiting for a rescan, so a busy folder is rescanned once it goes
//! quiet, or once a minute, not once per event batch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::events::{Change, EventBatch};

/// Seconds a folder must be left alone before it is rescanned.
pub const QUIET: i64 = 5;
/// Seconds after which a folder is rescanned even though it keeps changing.
pub const MAX_WAIT: i64 = 60;

#[derive(Debug)]
struct Entry {
    subtree: bool,
    first_seen: i64,
    last_seen: i64,
    /// The newest event id seen before this folder's first batch: a restart
    /// that replays from here cannot miss its events.
    floor: u64,
}

impl Entry {
    /// A folder last seen in the future (the clock moved back) is due, so it
    /// cannot wait for hours.
    fn is_due(&self, now: i64) -> bool {
        now < self.last_seen || now - self.last_seen >= QUIET || now - self.first_seen >= MAX_WAIT
    }

    fn seconds_left(&self, now: i64) -> i64 {
        if now < self.last_seen {
            return 0;
        }
        let due_at = (self.last_seen + QUIET).min(self.first_seen + MAX_WAIT);
        (due_at - now).max(0)
    }
}

/// Changed folders that have not been rescanned yet. Adding an event costs
/// one map lookup, however many events a folder gets.
#[derive(Debug, Default)]
pub struct Pending {
    entries: HashMap<PathBuf, Entry>,
    newest: u64,
}

impl Pending {
    /// An empty map. `stored_id` is the id in `fsevents_last_id`: every
    /// folder's floor starts from the newest id seen, and the daemon stores
    /// one before it starts the stream.
    pub fn new(stored_id: u64) -> Self {
        Pending {
            entries: HashMap::new(),
            newest: stored_id,
        }
    }

    /// Notes `batch`'s folders at time `now` (seconds). A folder already
    /// pending keeps its first-seen time, and a subtree change wins over a
    /// one-level one. `history_lost` is the caller's business and ignored.
    pub fn add(&mut self, batch: &EventBatch, now: i64) {
        let floor = self.newest;
        for change in &batch.changes {
            match self.entries.get_mut(&change.dir) {
                Some(entry) => {
                    entry.subtree |= change.subtree;
                    // The clock may have moved back since the folder was first seen.
                    entry.first_seen = entry.first_seen.min(now);
                    entry.last_seen = now;
                }
                None => {
                    self.entries.insert(
                        change.dir.clone(),
                        Entry {
                            subtree: change.subtree,
                            first_seen: now,
                            last_seen: now,
                            floor,
                        },
                    );
                }
            }
        }
        self.newest = self.newest.max(batch.last_id);
    }

    /// Removes the folders that were quiet for [`QUIET`], pending for
    /// [`MAX_WAIT`], or last seen after `now`. The batch's `last_id` is a
    /// floor, not the newest event: the lowest floor among the folders still
    /// pending, or the newest id seen when none are.
    pub fn take_due(&mut self, now: i64) -> Option<EventBatch> {
        self.take(|entry| entry.is_due(now))
    }

    /// Removes every folder, as [`Pending::take_due`] does for the due ones.
    pub fn take_all(&mut self) -> Option<EventBatch> {
        self.take(|_| true)
    }

    /// How long until the first folder is due: zero if one is, `None` when
    /// nothing is pending.
    pub fn wait(&self, now: i64) -> Option<Duration> {
        let seconds = self.entries.values().map(|e| e.seconds_left(now)).min()?;
        Some(Duration::from_secs(u64::try_from(seconds).unwrap_or(0)))
    }

    /// Forgets every folder, for a full rescan that covers them all.
    /// `last_id` is that batch's id: later floors start from it.
    pub fn clear(&mut self, last_id: u64) {
        self.entries.clear();
        self.newest = self.newest.max(last_id);
    }

    fn take(&mut self, due: impl Fn(&Entry) -> bool) -> Option<EventBatch> {
        let dirs: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|(_, entry)| due(entry))
            .map(|(dir, _)| dir.clone())
            .collect();
        if dirs.is_empty() {
            return None;
        }
        let changes = dirs
            .into_iter()
            .filter_map(|dir| {
                let entry = self.entries.remove(&dir)?;
                Some(Change {
                    dir,
                    subtree: entry.subtree,
                })
            })
            .collect();
        let last_id = self
            .entries
            .values()
            .map(|entry| entry.floor)
            .min()
            .unwrap_or(self.newest);
        Some(EventBatch {
            changes,
            last_id,
            history_lost: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{MAX_WAIT, Pending, QUIET};
    use crate::events::{Change, EventBatch};

    fn batch(dirs: &[(&str, bool)], id: u64) -> EventBatch {
        EventBatch {
            changes: dirs
                .iter()
                .map(|(dir, subtree)| Change {
                    dir: PathBuf::from(dir),
                    subtree: *subtree,
                })
                .collect(),
            last_id: id,
            history_lost: false,
        }
    }

    fn dirs(batch: &EventBatch) -> Vec<&str> {
        let mut dirs: Vec<_> = batch
            .changes
            .iter()
            .map(|c| c.dir.to_str().unwrap())
            .collect();
        dirs.sort_unstable();
        dirs
    }

    #[test]
    fn a_folder_is_due_only_after_it_was_quiet() {
        let mut p = Pending::new(0);
        p.add(&batch(&[("/a", false)], 1), 0);
        assert_eq!(p.take_due(QUIET - 1), None);
        assert_eq!(dirs(&p.take_due(QUIET).unwrap()), ["/a"]);
    }

    #[test]
    fn a_folder_that_never_goes_quiet_is_due_at_the_maximum_wait() {
        let mut p = Pending::new(0);
        for t in 0..MAX_WAIT {
            p.add(&batch(&[("/a", false)], t as u64 + 1), t);
        }
        assert_eq!(p.take_due(MAX_WAIT - 1), None);
        p.add(&batch(&[("/a", false)], 99), MAX_WAIT);
        assert_eq!(dirs(&p.take_due(MAX_WAIT).unwrap()), ["/a"]);
    }

    #[test]
    fn a_thousand_events_for_one_folder_are_one_change() {
        let mut p = Pending::new(0);
        for id in 1..=1000 {
            p.add(&batch(&[("/a", false)], id), 0);
        }
        assert_eq!(p.take_all().unwrap().changes.len(), 1);
    }

    #[test]
    fn a_subtree_change_wins_in_either_order() {
        for order in [[false, true], [true, false]] {
            let mut p = Pending::new(0);
            for (i, subtree) in order.into_iter().enumerate() {
                p.add(&batch(&[("/a", subtree)], i as u64 + 1), 0);
            }
            assert!(p.take_all().unwrap().changes[0].subtree, "{order:?}");
        }
    }

    #[test]
    fn the_stored_id_stays_below_what_is_still_pending() {
        let mut p = Pending::new(5);
        p.add(&batch(&[("/a", false)], 10), 0);
        p.add(&batch(&[("/b", false)], 20), 4);
        let first = p.take_due(QUIET).unwrap();
        assert_eq!(dirs(&first), ["/a"]);
        assert_eq!(first.last_id, 10);
        let second = p.take_due(4 + QUIET).unwrap();
        assert_eq!(dirs(&second), ["/b"]);
        assert_eq!(second.last_id, 20);
    }

    #[test]
    fn a_folder_seen_in_the_future_is_due() {
        let mut p = Pending::new(0);
        p.add(&batch(&[("/a", false)], 1), 1000);
        assert_eq!(dirs(&p.take_due(10).unwrap()), ["/a"]);
    }

    #[test]
    fn wait_counts_down_to_the_next_due_folder() {
        let mut p = Pending::new(0);
        assert_eq!(p.wait(0), None);
        p.add(&batch(&[("/a", false)], 1), 0);
        assert_eq!(p.wait(0), Some(Duration::from_secs(QUIET as u64)));
        assert_eq!(p.wait(3), Some(Duration::from_secs(QUIET as u64 - 3)));
        assert_eq!(p.wait(QUIET + 2), Some(Duration::ZERO));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut p = Pending::new(0);
        p.add(&batch(&[("/a", false)], 1), 0);
        p.clear(0);
        assert_eq!(p.take_all(), None);
        assert_eq!(p.wait(0), None);
    }

    #[test]
    fn clearing_raises_the_newest_id_the_next_floor_starts_from() {
        let mut p = Pending::new(500);
        p.clear(1000);
        p.add(&batch(&[("/a", false)], 1010), 3);
        p.add(&batch(&[("/b", false)], 1020), 0);
        let due = p.take_due(QUIET).unwrap();
        assert_eq!(dirs(&due), ["/b"]);
        assert_eq!(due.last_id, 1000);
    }

    #[test]
    fn a_folder_that_keeps_changing_after_the_clock_moved_back_is_still_applied() {
        let mut p = Pending::new(0);
        p.add(&batch(&[("/a", false)], 1), 10_000);
        for t in 1000..=1000 + MAX_WAIT {
            p.add(&batch(&[("/a", false)], 2), t);
        }
        assert_eq!(dirs(&p.take_due(1000 + MAX_WAIT).unwrap()), ["/a"]);
    }
}
