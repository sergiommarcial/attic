//! What the service learns about changes on disk, whatever reports them.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use crate::Result;

/// The `meta` key holding the id of the last file system event applied.
pub const LAST_EVENT_ID: &str = "fsevents_last_id";

/// The `meta` key counting the batches applied. The stored event id can stay
/// the same across applies (it trails the oldest pending folder), so the
/// terminal app watches this as well to know when to redraw.
pub const APPLIED_BATCHES: &str = "applied_batches";

/// A folder whose contents changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The folder (a file path is treated as its folder).
    pub dir: PathBuf,
    /// Everything below `dir` may have changed, not just its direct children.
    pub subtree: bool,
}

/// Changes reported together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBatch {
    /// The changed folders.
    pub changes: Vec<Change>,
    /// The newest event id in the batch, to resume from after a restart.
    pub last_id: u64,
    /// History was lost; nothing short of rescanning every root is right.
    pub history_lost: bool,
}

/// A source of [`EventBatch`]es.
pub trait EventSource {
    /// The next batch, or `None` when `timeout` passes without one.
    ///
    /// # Errors
    ///
    /// When the source has failed and will deliver nothing more.
    fn next_batch(&mut self, timeout: Duration) -> Result<Option<EventBatch>>;
}

/// Hands out batches given up front, then times out. For tests.
#[derive(Debug, Default)]
pub struct ScriptedEvents {
    batches: VecDeque<EventBatch>,
}

impl ScriptedEvents {
    /// A source that yields `batches` in order.
    pub fn new(batches: Vec<EventBatch>) -> Self {
        ScriptedEvents {
            batches: batches.into(),
        }
    }
}

impl EventSource for ScriptedEvents {
    fn next_batch(&mut self, _timeout: Duration) -> Result<Option<EventBatch>> {
        Ok(self.batches.pop_front())
    }
}
