//! The background service: keep the store current, and notify when there is
//! something worth cleaning.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::apply::apply_batch;
use crate::events::{EventSource, LAST_EVENT_ID};
use crate::notify::{Notifier, message, should_notify};
use crate::pending::Pending;
use crate::probe::NoProbe;
use crate::scan::{full_scan, scan_subtree};
use crate::store::Store;
use crate::suggest::{LIST_LEN, suggest};
use crate::{Config, Paths, Result};

/// How often suggestions are recomputed for notifications, in seconds.
const REFRESH_EVERY: i64 = 60;
/// How long `run` waits for events before checking the clock again.
const WAIT: Duration = Duration::from_secs(30);

/// What the caller should do after a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep stepping.
    Continue,
    /// The roots changed; start a new service (and event stream).
    Restart,
}

/// The service loop's state. Construct it with real or fake parts.
pub struct Service<'a> {
    store: Store,
    paths: Paths,
    cfg: Config,
    roots: Vec<PathBuf>,
    events: Box<dyn EventSource + 'a>,
    notifier: &'a dyn Notifier,
    clock: &'a dyn Fn() -> i64,
    pending: Pending,
    last_refresh: Option<i64>,
    changed: bool,
}

impl<'a> Service<'a> {
    /// A service over `store`, reading the config at `paths.config` (a bad
    /// one is reported on stderr and the defaults are used).
    ///
    /// # Errors
    ///
    /// [`crate::AtticError::Db`] when reading the stored event id fails.
    pub fn new(
        store: Store,
        paths: Paths,
        events: Box<dyn EventSource + 'a>,
        notifier: &'a dyn Notifier,
        clock: &'a dyn Fn() -> i64,
    ) -> Result<Self> {
        let cfg = load_config(&paths);
        let roots = roots(&cfg, &paths.home);
        let stored_id = store
            .meta(LAST_EVENT_ID)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Ok(Service {
            store,
            paths,
            cfg,
            roots,
            events,
            notifier,
            clock,
            pending: Pending::new(stored_id),
            last_refresh: None,
            changed: true,
        })
    }

    /// The config in use.
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Runs a full scan if none has finished yet.
    ///
    /// # Errors
    ///
    /// [`crate::AtticError::Db`] when the store fails.
    pub fn ensure_scanned(&mut self) -> Result<()> {
        if self.store.meta("full_scan_done_at")?.is_none() {
            full_scan(&mut self.store, &self.roots, &self.cfg, &mut |_| {})?;
        }
        self.reconcile()
    }

    /// Brings the store in line with the config and with what can be read
    /// now: roots without rows are scanned, rows of dropped roots and of
    /// excluded folders removed, and folders that could not be read are
    /// read again (Full Disk Access is often granted after the first scan).
    fn reconcile(&mut self) -> Result<()> {
        for top in self.store.tops()? {
            if !self.roots.iter().any(|r| top.starts_with(r)) {
                self.store.remove_subtree(&top)?;
            }
        }
        for excluded in &self.cfg.excludes {
            self.store.remove_subtree(excluded)?;
        }
        let mut rescan: Vec<PathBuf> = Vec::new();
        for root in &self.roots {
            if self.store.get(root)?.is_none() {
                rescan.push(root.clone());
            }
        }
        rescan.extend(
            self.store
                .unreadable()?
                .into_iter()
                .filter(|p| std::fs::read_dir(p).is_ok()),
        );
        for path in rescan {
            let entries = scan_subtree(&path, &self.cfg)?;
            self.store.replace_subtree(&path, &entries)?;
        }
        Ok(())
    }

    /// Takes at most one batch of events (waiting up to `timeout`, less when
    /// a folder is about to be due), applies the folders that have been
    /// quiet long enough or have waited a minute (see [`Pending`]), then
    /// refreshes the notification state if something changed and a minute
    /// has passed. A batch with lost history and one that touches the config
    /// folder are applied at once.
    ///
    /// # Errors
    ///
    /// When the event source or the store fails.
    pub fn step(&mut self, timeout: Duration) -> Result<Flow> {
        let wait = self
            .pending
            .wait((self.clock)())
            .map_or(timeout, |w| w.min(timeout));
        let mut touches_config = false;
        if let Some(batch) = self.events.next_batch(wait)? {
            let config_dir = self.paths.config.parent();
            touches_config = batch
                .changes
                .iter()
                .any(|c| Some(c.dir.as_path()) == config_dir);
            if batch.history_lost {
                self.pending.clear(batch.last_id);
                apply_batch(&mut self.store, &self.roots, &batch, &self.cfg)?;
                self.changed = true;
            } else {
                self.pending.add(&batch, (self.clock)());
            }
        }
        let now = (self.clock)();
        let due = if touches_config {
            self.pending.take_all()
        } else {
            self.pending.take_due(now)
        };
        if let Some(batch) = due {
            apply_batch(&mut self.store, &self.roots, &batch, &self.cfg)?;
            self.changed = true;
        }
        if touches_config {
            if self.reload_config() {
                return Ok(Flow::Restart);
            }
            self.reconcile()?;
        }
        // Nothing changed, nothing to recompute: an idle machine costs no
        // queries.
        if self.changed && self.last_refresh.is_none_or(|t| now - t >= REFRESH_EVERY) {
            self.refresh(now)?;
            self.last_refresh = Some(now);
            self.changed = false;
        }
        Ok(Flow::Continue)
    }

    /// Scans if needed, then steps until the roots change.
    ///
    /// # Errors
    ///
    /// When the event source or the store fails.
    pub fn run(mut self) -> Result<()> {
        self.ensure_scanned()?;
        while self.step(WAIT)? == Flow::Continue {}
        Ok(())
    }

    /// Returns whether the roots changed.
    fn reload_config(&mut self) -> bool {
        match Config::load(&self.paths.config, &self.paths.home) {
            Ok(cfg) => {
                let cfg = with_own_files_excluded(cfg, &self.paths);
                let roots = roots(&cfg, &self.paths.home);
                let changed = roots != self.roots;
                self.cfg = cfg;
                self.roots = roots;
                changed
            }
            Err(e) => {
                eprintln!("attic: {e}; keeping the previous config");
                false
            }
        }
    }

    fn refresh(&mut self, now: i64) -> Result<()> {
        let candidates = self.store.candidates(self.cfg.min_file_size)?;
        let total = suggest(
            &candidates,
            &self.paths.home,
            &self.roots,
            now,
            &NoProbe,
            LIST_LEN,
        )
        .total;
        let number =
            |key| -> Result<Option<i64>> { Ok(self.store.meta(key)?.and_then(|v| v.parse().ok())) };
        let mut last = match (number("last_notified_total")?, number("last_notified_at")?) {
            (Some(t), Some(at)) => Some((t.max(0) as u64, at)),
            _ => None,
        };
        if let Some((last_total, at)) = last
            && total < last_total
        {
            self.store
                .set_meta("last_notified_total", &total.to_string())?;
            last = Some((total, at));
        }
        if should_notify(total, last, now, &self.cfg) {
            if let Err(e) = self.notifier.send("attic", &message(total)) {
                eprintln!("attic: {e}");
                return Ok(());
            }
            self.store
                .set_meta("last_notified_total", &total.to_string())?;
            self.store.set_meta("last_notified_at", &now.to_string())?;
        }
        Ok(())
    }
}

/// The config file's settings (the defaults when it is bad, with the
/// reason on stderr), with attic's own folder excluded.
pub fn load_config(paths: &Paths) -> Config {
    let cfg = Config::load(&paths.config, &paths.home).unwrap_or_else(|e| {
        eprintln!("attic: {e}; using the defaults");
        Config::default()
    });
    with_own_files_excluded(cfg, paths)
}

/// Never scanned or suggested: attic's database folder, which churns while
/// it scans and is not the user's to clean, and the Trash, whose contents
/// are already on their way out.
fn with_own_files_excluded(mut cfg: Config, paths: &Paths) -> Config {
    if let Some(own) = paths.db.parent() {
        cfg.excludes.push(own.to_path_buf());
    }
    cfg.excludes.push(paths.home.join(".Trash"));
    cfg
}

/// The home folder plus the configured roots, without repeats or roots
/// inside another root.
pub fn roots(cfg: &Config, home: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = Vec::new();
    for root in std::iter::once(home.to_path_buf()).chain(cfg.roots.iter().cloned()) {
        if !all.iter().any(|r| root.starts_with(r)) {
            all.retain(|r| !r.starts_with(&root));
            all.push(root);
        }
    }
    all
}
