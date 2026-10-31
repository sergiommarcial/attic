//! The terminal app.

pub mod app;
pub mod ui;

use std::time::Duration;

use attic_core::disk;
use attic_core::events::LAST_EVENT_ID;
use attic_core::probe::{CachedProbe, SystemProbe};
use attic_core::scan::now;
use attic_core::service::{load_config, roots};
use attic_core::store::Store;
use attic_core::trash::FinderTrasher;
use attic_core::{AtticError, Paths, Result};
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use crate::commands::agent_loaded;
use app::{App, Context, Deps, Flow};

/// How often the app checks whether the service changed anything.
const REFRESH: Duration = Duration::from_secs(5);
/// How long a Spotlight or git answer is reused.
const PROBE_TTL: Duration = Duration::from_secs(300);

/// Runs the terminal app until `q`.
///
/// # Errors
///
/// When the store or the terminal fails.
pub fn run(paths: &Paths) -> Result<()> {
    let cfg = load_config(paths);
    let mut ctx = Context {
        roots: roots(&cfg, &paths.home),
        paths: paths.clone(),
        cfg,
        now: now(),
        disk: disk::usage(&paths.home)?,
        agent_loaded: agent_loaded(),
    };
    let mut store = Store::open(&paths.db)?;
    let probe = CachedProbe::new(SystemProbe, PROBE_TTL);
    let trasher = FinderTrasher;
    let mut app = App::load(&store, &ctx)?;
    let mut probed = false;

    let mut terminal = ratatui::try_init().map_err(|e| AtticError::Command {
        program: "terminal".into(),
        message: format!("none to draw in ({e}). `attic status` works without one."),
    })?;
    let result = (|| -> Result<()> {
        let terminal_error = |e: std::io::Error| AtticError::io("terminal", e);
        let mut seen = stamp(&store)?;
        loop {
            terminal
                .draw(|f| ui::draw(f, &app, &ctx.paths.home))
                .map_err(terminal_error)?;
            if !probed {
                // The first frame went out without waiting on Spotlight and
                // git; now fill in dates and warnings.
                probed = true;
                app.reload(&store, &ctx, &probe)?;
                continue;
            }
            if event::poll(REFRESH).map_err(terminal_error)? {
                if let Event::Key(key) = event::read().map_err(terminal_error)?
                    && key.kind == KeyEventKind::Press
                {
                    let mut deps = Deps {
                        store: &mut store,
                        trasher: &trasher,
                        probe: &probe,
                    };
                    if app.on_key(key.code, &ctx, &mut deps)? == Flow::Quit {
                        return Ok(());
                    }
                }
            } else {
                let current = stamp(&store)?;
                if current != seen {
                    seen = current;
                    ctx.now = now();
                    ctx.disk = disk::usage(&ctx.paths.home)?;
                    ctx.agent_loaded = agent_loaded();
                    app.reload(&store, &ctx, &probe)?;
                }
            }
        }
    })();
    ratatui::restore();
    result
}

/// Changes whenever the service has written something worth redrawing.
fn stamp(store: &Store) -> Result<[Option<String>; 3]> {
    Ok([
        store.meta(LAST_EVENT_ID)?,
        store.meta("full_scan_done_at")?,
        store.meta("scan_progress")?,
    ])
}
