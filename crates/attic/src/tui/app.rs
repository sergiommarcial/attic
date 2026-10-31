//! The terminal app's state and what each key does, kept apart from
//! drawing so it can be tested without a terminal.

use std::path::{Path, PathBuf};

use attic_core::disk::{self, DiskUsage};
use attic_core::probe::{NoProbe, Probe};
use attic_core::store::{Entry, Kind, Store};
use attic_core::suggest::{
    LIST_LEN, Suggestion, Suggestions, Warning, assess, is_protected, suggest,
};
use attic_core::trash::{TrashOutcome, Trasher, move_to_trash};
use attic_core::units::human;
use attic_core::{Config, Paths, Result};
use ratatui::crossterm::event::KeyCode;

use super::ui::display_path;

/// How many rows the "Largest folders" list shows.
const LARGEST_LEN: usize = 10;
/// How many unreadable folders the `u` dialog lists.
const UNREADABLE_SHOWN: usize = 15;

/// What the app was started with and refreshes now and then.
#[derive(Debug, Clone)]
pub struct Context {
    /// Where attic's files are.
    pub paths: Paths,
    /// The scan roots.
    pub roots: Vec<PathBuf>,
    /// The config in use.
    pub cfg: Config,
    /// Unix seconds, now.
    pub now: i64,
    /// The home volume's usage.
    pub disk: DiskUsage,
    /// Whether the background service is loaded.
    pub agent_loaded: bool,
}

/// What a key press may use.
pub struct Deps<'a> {
    /// The database.
    pub store: &'a mut Store,
    /// Moves things to the Trash.
    pub trasher: &'a dyn Trasher,
    /// Answers last-used and git questions.
    pub probe: &'a dyn Probe,
}

/// The screen being shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// Disk usage, largest folders and suggestions.
    Overview,
    /// A folder's contents.
    Browse(PathBuf),
    /// Asking before moving `target` to the Trash.
    Confirm {
        /// What would be moved.
        target: PathBuf,
        /// Its size.
        size: u64,
        /// Why it needs a second `y`.
        warnings: Vec<Warning>,
        /// The first `y` was pressed.
        second: bool,
    },
    /// A result to read; any key goes back.
    Message(String),
}

/// Which overview list has the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Largest (or oldest) folders.
    Largest,
    /// Suggestions.
    Suggestions,
}

/// How folder lists are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// Largest first.
    Size,
    /// Least recently used first.
    Oldest,
}

/// Whether to keep running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Continue,
    /// Leave the app.
    Quit,
}

/// One selectable line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The folder or file.
    pub path: PathBuf,
    /// Folder or file.
    pub kind: Kind,
    /// Its size.
    pub size: u64,
    /// Days since last used.
    pub idle_days: u64,
    /// 0 for a top-level suggestion, 1 for one listed under it.
    pub depth: u8,
    /// Rebuilt by its tool when needed.
    pub rebuildable: bool,
    /// Has a warning.
    pub warned: bool,
}

/// Everything the screen shows.
#[derive(Debug)]
pub struct App {
    /// The current screen.
    pub view: View,
    back: Vec<View>,
    /// Which overview list has the cursor.
    pub focus: Focus,
    /// How folder lists are ordered.
    pub sort: Sort,
    /// The cursor, within [`App::rows`].
    pub selected: usize,
    /// Notes shown above everything (no scan, service down, ...).
    pub banners: Vec<String>,
    /// The home volume's usage.
    pub disk: DiskUsage,
    /// The usage of the browsed folder's volume, when it is not home's.
    pub browse_disk: Option<DiskUsage>,
    /// The largest folders.
    pub largest: Vec<Row>,
    /// The least recently used of the folders big enough to suggest.
    pub oldest: Vec<Row>,
    /// The suggestions and their total.
    pub suggestions: Suggestions,
    suggestion_rows: Vec<Row>,
    /// The browsed folder's children.
    pub browse: Vec<Row>,
    /// The browsed folder's small files: count and bytes.
    pub other_files: Option<(u64, u64)>,
    /// Folders that could not be read.
    pub unreadable: Vec<PathBuf>,
}

impl App {
    /// Builds the overview from the store alone, without asking Spotlight or
    /// git anything, so the first frame is quick; [`App::reload`] with a
    /// real probe fills in last-used dates and warnings.
    ///
    /// # Errors
    ///
    /// [`attic_core::AtticError::Db`] when the store fails.
    pub fn load(store: &Store, ctx: &Context) -> Result<App> {
        let mut app = App {
            view: View::Overview,
            back: Vec::new(),
            focus: Focus::Largest,
            sort: Sort::Size,
            selected: 0,
            banners: Vec::new(),
            disk: ctx.disk,
            browse_disk: None,
            largest: Vec::new(),
            oldest: Vec::new(),
            suggestions: Suggestions::default(),
            suggestion_rows: Vec::new(),
            browse: Vec::new(),
            other_files: None,
            unreadable: Vec::new(),
        };
        app.reload(store, ctx, &NoProbe)?;
        Ok(app)
    }

    /// Reads everything again (after a change on disk or a cleanup).
    ///
    /// # Errors
    ///
    /// [`attic_core::AtticError::Db`] when the store fails.
    pub fn reload(&mut self, store: &Store, ctx: &Context, probe: &dyn Probe) -> Result<()> {
        self.banners = banners(store, ctx)?;
        self.unreadable = store.unreadable()?;
        self.disk = ctx.disk;
        let is_root = |e: &Entry| ctx.roots.contains(&e.path);
        self.largest = store
            .largest_dirs(LARGEST_LEN + ctx.roots.len())?
            .iter()
            .filter(|e| !is_root(e))
            .take(LARGEST_LEN)
            .map(|e| row(e, ctx.now))
            .collect();
        let candidates = store.candidates(ctx.cfg.min_file_size)?;
        let mut oldest: Vec<Row> = candidates
            .iter()
            .filter(|e| e.kind == Kind::Dir && !is_root(e))
            .map(|e| row(e, ctx.now))
            .collect();
        sort_rows(&mut oldest, Sort::Oldest);
        oldest.truncate(LARGEST_LEN);
        self.oldest = oldest;
        self.suggestions = suggest(
            &candidates,
            &ctx.paths.home,
            &ctx.roots,
            ctx.now,
            probe,
            LIST_LEN,
        );
        self.suggestion_rows = self
            .suggestions
            .items
            .iter()
            .flat_map(|s| {
                std::iter::once(suggestion_row(s, 0))
                    .chain(s.nested.iter().map(|n| suggestion_row(n, 1)))
            })
            .collect();
        self.reload_browse(store, ctx)
    }

    fn reload_browse(&mut self, store: &Store, ctx: &Context) -> Result<()> {
        if let Some(dir) = self.browsed().map(Path::to_path_buf) {
            self.browse = store
                .children(&dir)?
                .iter()
                .map(|e| row(e, ctx.now))
                .collect();
            sort_rows(&mut self.browse, self.sort);
            self.other_files = store
                .get(&dir)?
                .filter(|e| e.small_files > 0)
                .map(|e| (e.small_files, e.small_bytes));
            self.browse_disk = (!dir.starts_with(&ctx.paths.home))
                .then(|| disk::usage(&dir).ok())
                .flatten();
        }
        self.selected = self.selected.min(self.rows().len().saturating_sub(1));
        Ok(())
    }

    /// The folder being browsed, also while a dialog is over it.
    pub fn browsed(&self) -> Option<&Path> {
        match self.base_view() {
            View::Browse(p) => Some(p),
            _ => None,
        }
    }

    /// The list screen under any dialog.
    pub fn base_view(&self) -> &View {
        std::iter::once(&self.view)
            .chain(self.back.iter().rev())
            .find(|v| matches!(v, View::Overview | View::Browse(_)))
            .unwrap_or(&View::Overview)
    }

    /// Whether a list (not a dialog) has the keyboard.
    pub fn view_is_list(&self) -> bool {
        matches!(self.view, View::Overview | View::Browse(_))
    }

    /// The rows the cursor moves over.
    pub fn rows(&self) -> &[Row] {
        match self.base_view() {
            View::Browse(_) => &self.browse,
            _ => self.overview_rows(self.focus),
        }
    }

    /// One of the two overview lists.
    pub fn overview_rows(&self, focus: Focus) -> &[Row] {
        match (focus, self.sort) {
            (Focus::Largest, Sort::Size) => &self.largest,
            (Focus::Largest, Sort::Oldest) => &self.oldest,
            (Focus::Suggestions, _) => &self.suggestion_rows,
        }
    }

    /// Handles one key.
    ///
    /// # Errors
    ///
    /// [`attic_core::AtticError::Db`] when reloading from the store fails;
    /// a failed cleanup is shown as a message, not returned.
    pub fn on_key(&mut self, key: KeyCode, ctx: &Context, deps: &mut Deps) -> Result<Flow> {
        match self.view.clone() {
            View::Confirm {
                target,
                size,
                warnings,
                second,
            } => {
                if key != KeyCode::Char('y') {
                    self.go_back();
                } else if !warnings.is_empty() && !second {
                    self.view = View::Confirm {
                        target,
                        size,
                        warnings,
                        second: true,
                    };
                } else {
                    self.clean(&target, ctx, deps)?;
                }
            }
            View::Message(_) => self.go_back(),
            View::Overview | View::Browse(_) => return self.on_list_key(key, ctx, deps),
        }
        Ok(Flow::Continue)
    }

    fn on_list_key(&mut self, key: KeyCode, ctx: &Context, deps: &mut Deps) -> Result<Flow> {
        let len = self.rows().len();
        match key {
            KeyCode::Char('q') => return Ok(Flow::Quit),
            KeyCode::Char('u') if !self.unreadable.is_empty() => {
                let mut text = String::from("attic could not read these folders:");
                for path in self.unreadable.iter().take(UNREADABLE_SHOWN) {
                    text.push_str(&format!("\n  {}", display_path(path, &ctx.paths.home)));
                }
                if self.unreadable.len() > UNREADABLE_SHOWN {
                    let more = self.unreadable.len() - UNREADABLE_SHOWN;
                    text.push_str(&format!("\n  and {more} more (`attic status` lists all)"));
                }
                text.push_str(&format!("\n\n{}", crate::commands::full_disk_access_hint()));
                self.back.push(self.view.clone());
                self.view = View::Message(text);
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(len.saturating_sub(1));
            }
            KeyCode::Tab if self.view == View::Overview => {
                self.focus = match self.focus {
                    Focus::Largest => Focus::Suggestions,
                    Focus::Suggestions => Focus::Largest,
                };
                self.selected = 0;
            }
            KeyCode::Char('s') => {
                self.sort = match self.sort {
                    Sort::Size => Sort::Oldest,
                    Sort::Oldest => Sort::Size,
                };
                sort_rows(&mut self.browse, self.sort);
                self.selected = 0;
            }
            KeyCode::Enter => {
                if let Some(row) = self
                    .rows()
                    .get(self.selected)
                    .filter(|r| r.kind == Kind::Dir)
                {
                    let dir = row.path.clone();
                    self.back.push(self.view.clone());
                    self.view = View::Browse(dir);
                    self.selected = 0;
                    self.reload_browse(deps.store, ctx)?;
                }
            }
            KeyCode::Backspace | KeyCode::Esc => {
                if let Some(previous) = self.back.pop() {
                    self.view = previous;
                    self.selected = 0;
                    self.reload_browse(deps.store, ctx)?;
                }
            }
            KeyCode::Char('c') => {
                if let Some(row) = self.rows().get(self.selected).cloned() {
                    if is_protected(&row.path, &ctx.paths.home, &ctx.roots) {
                        self.back.push(self.view.clone());
                        self.view = View::Message(format!(
                            "{} is a top-level folder; attic won't move it to the Trash. Open it with Enter and clean what's inside.",
                            display_path(&row.path, &ctx.paths.home)
                        ));
                        return Ok(Flow::Continue);
                    }
                    let mut item = Suggestion {
                        path: row.path.clone(),
                        kind: row.kind,
                        size: row.size,
                        idle_days: row.idle_days,
                        rebuildable: row.rebuildable,
                        warnings: Vec::new(),
                        nested: Vec::new(),
                    };
                    assess(&mut item, &ctx.paths.home, ctx.now, deps.probe);
                    self.back.push(self.view.clone());
                    self.view = View::Confirm {
                        target: row.path,
                        size: row.size,
                        warnings: item.warnings,
                        second: false,
                    };
                }
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn clean(&mut self, target: &Path, ctx: &Context, deps: &mut Deps) -> Result<()> {
        let shown = display_path(target, &ctx.paths.home);
        let message = match move_to_trash(deps.store, deps.trasher, target) {
            Ok(TrashOutcome::Moved { bytes }) => format!(
                "Moved to the Trash: {shown} ({}). It frees space once the Trash is emptied.",
                human(bytes)
            ),
            Ok(TrashOutcome::AlreadyGone { .. }) => {
                format!("{shown} was already gone; removed from the list.")
            }
            Err(e) => format!("Could not move {shown} to the Trash: {e}"),
        };
        self.view = View::Message(message);
        self.reload(deps.store, ctx, deps.probe)
    }

    fn go_back(&mut self) {
        self.view = self.back.pop().unwrap_or(View::Overview);
    }

    /// Puts the cursor on `path` in whichever list holds it.
    #[cfg(test)]
    pub fn select(&mut self, path: &Path) -> bool {
        for focus in [Focus::Largest, Focus::Suggestions] {
            if self.view == View::Overview {
                self.focus = focus;
            }
            if let Some(i) = self.rows().iter().position(|r| r.path == path) {
                self.selected = i;
                return true;
            }
        }
        false
    }
}

fn banners(store: &Store, ctx: &Context) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let progress = store.meta("scan_progress")?.filter(|p| !p.is_empty());
    if let Some(p) = &progress {
        let mut parts = p.splitn(3, ' ');
        let dirs = parts.next().unwrap_or("0");
        let bytes = parts.next().and_then(|b| b.parse().ok()).unwrap_or(0);
        out.push(format!(
            "Scanning: {dirs} folders, {} so far.",
            human(bytes)
        ));
    } else if store.meta("full_scan_done_at")?.is_none() {
        out.push(
            "No scan yet. Run `attic scan`, or `attic install` to run attic in the background."
                .into(),
        );
    }
    if !ctx.agent_loaded {
        out.push("The background service isn't running. Run `attic install`.".into());
    }
    let unreadable = store.unreadable()?.len();
    if unreadable > 0 {
        out.push(format!(
            "{unreadable} folders could not be read (press u to list them). {}",
            crate::commands::full_disk_access_hint()
        ));
    }
    Ok(out)
}

fn row(e: &Entry, now: i64) -> Row {
    let used = e.last_used.unwrap_or(e.newest_mtime);
    Row {
        path: e.path.clone(),
        kind: e.kind,
        size: e.size,
        idle_days: (now - used).max(0) as u64 / 86_400,
        depth: 0,
        rebuildable: false,
        warned: false,
    }
}

fn suggestion_row(s: &Suggestion, depth: u8) -> Row {
    Row {
        path: s.path.clone(),
        kind: s.kind,
        size: s.size,
        idle_days: s.idle_days,
        depth,
        rebuildable: s.rebuildable,
        warned: !s.warnings.is_empty(),
    }
}

fn sort_rows(rows: &mut [Row], sort: Sort) {
    match sort {
        Sort::Size => rows.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path))),
        Sort::Oldest => rows.sort_by(|a, b| {
            b.idle_days
                .cmp(&a.idle_days)
                .then_with(|| b.size.cmp(&a.size))
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    use attic_core::disk::DiskUsage;
    use attic_core::probe::FakeProbe;
    use attic_core::scan::{full_scan, now};
    use attic_core::store::Store;
    use attic_core::trash::RecordingTrasher;
    use attic_core::{Config, Paths};
    use ratatui::crossterm::event::KeyCode;

    pub(crate) struct Fixture {
        pub _dir: tempfile::TempDir,
        pub home: PathBuf,
        pub store: Store,
        pub trasher: RecordingTrasher,
        pub probe: FakeProbe,
        pub ctx: Context,
    }

    fn write(path: &Path, bytes: usize, old: bool) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        f.write_all(&vec![5u8; bytes]).unwrap();
        if old {
            f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
                .unwrap();
        }
    }

    /// home/old/{a.bin 600k old, small.txt 1k old}, home/fresh/b.bin 300k new.
    pub(crate) fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap().join("home");
        write(&home.join("old/a.bin"), 600_000, true);
        write(&home.join("old/small.txt"), 1_000, true);
        write(&home.join("fresh/b.bin"), 300_000, false);
        // Creating files bumped old/'s own mtime; make the folder old too.
        File::open(home.join("old"))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
            .unwrap();
        let paths = Paths::from_home(&home);
        let cfg = Config {
            min_file_size: 100_000,
            ..Config::default()
        };
        let mut store = Store::open_in_memory().unwrap();
        full_scan(&mut store, std::slice::from_ref(&home), &cfg, &mut |_| {}).unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let ctx = Context {
            roots: vec![home.clone()],
            paths,
            cfg,
            now: now(),
            disk: DiskUsage {
                total: 1_000_000_000_000,
                available: 490_000_000_000,
            },
            agent_loaded: true,
        };
        Fixture {
            _dir: dir,
            home,
            store,
            trasher: RecordingTrasher::new(bin),
            probe: FakeProbe::default(),
            ctx,
        }
    }

    impl Fixture {
        pub(crate) fn app(&self) -> App {
            let mut app = App::load(&self.store, &self.ctx).unwrap();
            app.reload(&self.store, &self.ctx, &self.probe).unwrap();
            app
        }

        fn key(&mut self, app: &mut App, key: KeyCode) -> Flow {
            let mut deps = Deps {
                store: &mut self.store,
                trasher: &self.trasher,
                probe: &self.probe,
            };
            app.on_key(key, &self.ctx, &mut deps).unwrap()
        }
    }

    #[test]
    fn enter_opens_browse_and_back_returns() {
        let mut f = fixture();
        let mut app = f.app();
        assert!(app.select(&f.home.join("old")));
        f.key(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::Browse(f.home.join("old")));
        f.key(&mut app, KeyCode::Backspace);
        assert_eq!(app.view, View::Overview);
    }

    #[test]
    fn tab_switches_lists() {
        let mut f = fixture();
        let mut app = f.app();
        let before = app.focus;
        f.key(&mut app, KeyCode::Tab);
        assert_ne!(app.focus, before);
    }

    #[test]
    fn s_sorts_by_least_recently_used() {
        let mut f = fixture();
        let mut app = f.app();
        assert!(app.select(&f.home.join("old")));
        f.key(&mut app, KeyCode::Enter);
        let mut app2 = app;
        app2.view = View::Browse(f.home.clone());
        app2.reload(&f.store, &f.ctx, &f.probe).unwrap();
        let first = |a: &App| a.rows()[0].path.clone();
        // By size, old/ (600k) leads; fresh/ has 300k.
        assert_eq!(first(&app2), f.home.join("old"));
        f.key(&mut app2, KeyCode::Char('s'));
        assert_eq!(app2.sort, Sort::Oldest);
        assert_eq!(first(&app2), f.home.join("old"));
        write(&f.home.join("fresh/c.bin"), 900_000, true);
        let mut f2 = f;
        attic_core::scan::full_scan(
            &mut f2.store,
            std::slice::from_ref(&f2.home),
            &f2.ctx.cfg,
            &mut |_| {},
        )
        .unwrap();
        app2.reload(&f2.store, &f2.ctx, &f2.probe).unwrap();
        // fresh/ is now the largest, but old/ is still the least recently used.
        assert_eq!(first(&app2), f2.home.join("old"));
        f2.key(&mut app2, KeyCode::Char('s'));
        assert_eq!(first(&app2), f2.home.join("fresh"));
    }

    #[test]
    fn clean_without_warnings_needs_one_y() {
        let mut f = fixture();
        let mut app = f.app();
        let target = f.home.join("old/a.bin");
        assert!(app.select(&target));
        f.key(&mut app, KeyCode::Char('c'));
        assert!(
            matches!(&app.view, View::Confirm { target: t, second: false, .. } if *t == target)
        );
        f.key(&mut app, KeyCode::Char('y'));
        assert_eq!(f.trasher.moved(), std::slice::from_ref(&target));
        assert!(
            matches!(&app.view, View::Message(m) if m.starts_with("Moved to the Trash")),
            "{:?}",
            app.view
        );
        assert!(f.store.get(&target).unwrap().is_none());
    }

    #[test]
    fn clean_with_warnings_needs_two_y() {
        let mut f = fixture();
        f.probe.unsaved.insert(f.home.join("old"));
        let mut app = f.app();
        assert!(app.select(&f.home.join("old")));
        f.key(&mut app, KeyCode::Char('c'));
        f.key(&mut app, KeyCode::Char('y'));
        assert!(
            matches!(&app.view, View::Confirm { second: true, .. }),
            "{:?}",
            app.view
        );
        assert!(f.trasher.moved().is_empty());
        f.key(&mut app, KeyCode::Char('y'));
        assert_eq!(f.trasher.moved(), [f.home.join("old")]);
    }

    #[test]
    fn other_key_cancels() {
        let mut f = fixture();
        let mut app = f.app();
        assert!(app.select(&f.home.join("old/a.bin")));
        f.key(&mut app, KeyCode::Char('c'));
        assert_eq!(f.key(&mut app, KeyCode::Char('q')), Flow::Continue);
        assert_eq!(app.view, View::Overview);
        assert!(f.trasher.moved().is_empty());
    }

    #[test]
    fn already_gone_message() {
        let mut f = fixture();
        let mut app = f.app();
        let target = f.home.join("old/a.bin");
        assert!(app.select(&target));
        fs::remove_file(&target).unwrap();
        f.key(&mut app, KeyCode::Char('c'));
        f.key(&mut app, KeyCode::Char('y'));
        assert!(
            matches!(&app.view, View::Message(m) if m.contains("was already gone")),
            "{:?}",
            app.view
        );
        f.key(&mut app, KeyCode::Char('x'));
        assert_eq!(app.view, View::Overview);
    }

    #[test]
    fn no_scan_banner() {
        let f = fixture();
        let empty = Store::open_in_memory().unwrap();
        let app = App::load(&empty, &f.ctx).unwrap();
        assert!(
            app.banners.iter().any(|b| b.starts_with("No scan yet")),
            "{:?}",
            app.banners
        );
    }

    #[test]
    fn service_down_banner() {
        let mut f = fixture();
        f.ctx.agent_loaded = false;
        let app = f.app();
        assert!(
            app.banners.iter().any(|b| b.contains("attic install")),
            "{:?}",
            app.banners
        );
    }

    #[test]
    fn standard_folders_cannot_be_cleaned() {
        let mut f = fixture();
        fs::create_dir_all(f.home.join("Music/old")).unwrap();
        fs::write(f.home.join("Music/old/song.bin"), vec![1u8; 400_000]).unwrap();
        attic_core::scan::full_scan(
            &mut f.store,
            std::slice::from_ref(&f.home),
            &f.ctx.cfg,
            &mut |_| {},
        )
        .unwrap();
        let mut app = f.app();
        assert!(app.select(&f.home.join("Music")));
        f.key(&mut app, KeyCode::Char('c'));
        assert!(
            matches!(&app.view, View::Message(m) if m.contains("won't move")),
            "{:?}",
            app.view
        );
        f.key(&mut app, KeyCode::Char('y'));
        assert!(f.trasher.moved().is_empty());
    }

    #[test]
    fn u_lists_unreadable_folders() {
        let mut f = fixture();
        let locked = f.home.join("locked");
        let mut row = f.store.get(&f.home.join("old")).unwrap().unwrap();
        row.path = locked.clone();
        row.size = 0;
        row.flags.insert(attic_core::store::Flags::UNREADABLE);
        f.store.transaction(|tx| tx.put_entry(&row)).unwrap();
        let mut app = f.app();
        assert!(
            app.banners.iter().any(|b| b.contains("press u")),
            "{:?}",
            app.banners
        );
        f.key(&mut app, KeyCode::Char('u'));
        assert!(
            matches!(&app.view, View::Message(m) if m.contains("~/locked")),
            "{:?}",
            app.view
        );
    }

    #[test]
    fn q_quits() {
        let mut f = fixture();
        let mut app = f.app();
        assert_eq!(f.key(&mut app, KeyCode::Char('q')), Flow::Quit);
    }
}
