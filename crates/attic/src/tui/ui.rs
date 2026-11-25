//! Drawing the app.

use std::path::Path;

use attic_core::disk::DiskUsage;
use attic_core::store::Kind;
use attic_core::suggest::Warning;
use attic_core::units::{age, human};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::app::{App, Focus, Row, Sort, View};

const BAR_CELLS: usize = 20;
const KEYS: &str = "↑↓ move · Tab switch list · Enter open · Backspace back · s sort · c clean · u unreadable · q quit";

/// Draws the whole screen.
pub fn draw(frame: &mut Frame, app: &App, home: &Path) {
    let banner_lines = app.banners.len() as u16;
    let [banners, disk, main, footer] = Layout::vertical([
        Constraint::Length(banner_lines),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    let bold = Style::default().add_modifier(Modifier::BOLD);
    frame.render_widget(
        Paragraph::new(
            app.banners
                .iter()
                .map(|b| Line::styled(b.as_str(), bold))
                .collect::<Vec<_>>(),
        ),
        banners,
    );
    frame.render_widget(Paragraph::new(disk_line("Disk", &app.disk)), disk);

    match app.base_view() {
        View::Browse(dir) => draw_browse(frame, app, dir, home, main),
        _ => draw_overview(frame, app, home, main),
    }
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Moving to the Trash frees space once the Trash is emptied."),
            Line::from(KEYS),
        ]),
        footer,
    );

    match &app.view {
        View::Confirm {
            target,
            size,
            warnings,
            second,
        } => {
            let mut lines = vec![
                Line::styled("Move to the Trash?", bold),
                Line::from(""),
                Line::from(display_path(target, home)),
                Line::from(human(*size)),
            ];
            for w in warnings {
                lines.push(Line::styled(format!("! {}", warning_text(*w)), bold));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(if *second {
                "Press y again to move it to the Trash. Any other key cancels."
            } else {
                "y: move it to the Trash · any other key: cancel"
            }));
            dialog(frame, lines);
        }
        View::Message(text) => {
            let mut lines: Vec<Line> = text.lines().map(Line::from).collect();
            lines.push(Line::from(""));
            lines.push(Line::from("Press any key."));
            dialog(frame, lines);
        }
        _ => {}
    }
}

fn draw_overview(frame: &mut Frame, app: &App, home: &Path, area: Rect) {
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Fill(1)]).areas(area);
    let title = match app.sort {
        Sort::Size => "Largest folders",
        Sort::Oldest => "Largest folders · least recently used first",
    };
    list(frame, app, Focus::Largest, title, home, top);
    let title = format!("Could free ~{}", human(app.suggestions.total));
    list(frame, app, Focus::Suggestions, &title, home, bottom);
}

fn list(frame: &mut Frame, app: &App, focus: Focus, title: &str, home: &Path, area: Rect) {
    let rows = app.overview_rows(focus);
    let width = area.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| ListItem::new(row_text(r, home, width, focus)))
        .collect();
    let focused = app.view_is_list() && app.focus == focus;
    render_list(frame, items, title, focused.then_some(app.selected), area);
}

fn draw_browse(frame: &mut Frame, app: &App, dir: &Path, home: &Path, area: Rect) {
    let extra = u16::from(app.browse_disk.is_some()) + u16::from(app.other_files.is_some());
    let [head, body, tail] = Layout::vertical([
        Constraint::Length(u16::from(app.browse_disk.is_some())),
        Constraint::Fill(1),
        Constraint::Length(extra.min(1)),
    ])
    .areas(area);
    if let Some(usage) = &app.browse_disk {
        frame.render_widget(Paragraph::new(disk_line("Volume", usage)), head);
    }
    let width = body.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = app
        .browse
        .iter()
        .map(|r| ListItem::new(row_text(r, home, width, Focus::Largest)))
        .collect();
    let sort = match app.sort {
        Sort::Size => "",
        Sort::Oldest => " · least recently used first",
    };
    let title = format!("{}{sort}", display_path(dir, home));
    render_list(
        frame,
        items,
        &title,
        app.view_is_list().then_some(app.selected),
        body,
    );
    if let Some((count, bytes)) = app.other_files {
        let s = if count == 1 { "" } else { "s" };
        frame.render_widget(
            Paragraph::new(format!("  {count} other file{s}, {}", human(bytes))),
            tail,
        );
    }
}

fn render_list(
    frame: &mut Frame,
    items: Vec<ListItem>,
    title: &str,
    selected: Option<usize>,
    area: Rect,
) {
    let widget = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.to_string()),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(widget, area, &mut state);
}

fn row_text(r: &Row, home: &Path, width: usize, focus: Focus) -> String {
    let indent = "  ".repeat(r.depth as usize);
    let marker = if focus == Focus::Suggestions {
        "▸ "
    } else {
        ""
    };
    // Fixed-width tags keep the size and age columns aligned.
    let tags = format!(
        "{:<11} {}",
        if r.rebuildable { "rebuildable" } else { "" },
        if r.warned { "!" } else { " " }
    );
    let used = match (focus, r.idle_days) {
        (Focus::Suggestions, 0) => "used today".to_string(),
        (Focus::Suggestions, days) => format!("untouched {}", age(days)),
        (_, days) => age(days),
    };
    let kind = if r.kind == Kind::Dir { "/" } else { "" };
    let tags = if focus == Focus::Suggestions {
        format!("  {tags}")
    } else {
        String::new()
    };
    let right = format!("{:>8}  {used:>18}{tags}", human(r.size));
    let left = format!("{indent}{marker}{}{kind}", display_path(&r.path, home));
    let room = width.saturating_sub(right.width() + 1);
    let left = truncate(&left, room);
    let pad = room.saturating_sub(left.width());
    format!("{left}{} {right}", " ".repeat(pad))
}

/// Keeps the end of `text` (the file name matters most) within `room`
/// terminal cells, with `…` in front when something was cut.
fn truncate(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    let mut kept = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > room {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}
fn disk_line(label: &str, usage: &DiskUsage) -> String {
    let fraction = if usage.total == 0 {
        0.0
    } else {
        usage.used() as f64 / usage.total as f64
    };
    let filled = (fraction * BAR_CELLS as f64).round() as usize;
    format!(
        "{label}  {}{}  {} used of {} ({:.0}%)",
        "█".repeat(filled),
        "░".repeat(BAR_CELLS - filled.min(BAR_CELLS)),
        human(usage.used()),
        human(usage.total),
        fraction * 100.0
    )
}

fn dialog(frame: &mut Frame, lines: Vec<Line>) {
    let area = frame.area();
    let width = area.width.saturating_sub(8).min(80);
    let height = (lines.len() as u16 + 2).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL)),
        rect,
    );
}

fn warning_text(w: Warning) -> &'static str {
    match w {
        Warning::UnsavedRepo => {
            "Holds a git repository with uncommitted, stashed or unpushed work."
        }
        Warning::LibraryBundle => "A macOS library (Photos, Mail or Messages).",
        Warning::RecentlyUsed => "Used in the last week.",
    }
}

/// A path for one line of the screen: `~` for the home folder, control
/// characters (a newline in a name) shown as `?`.
pub fn display_path(path: &Path, home: &Path) -> String {
    let text = match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.to_string_lossy()),
        Err(_) => path.to_string_lossy().into_owned(),
    };
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::tests::fixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &crate::tui::app::App, home: &std::path::Path) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| draw(f, app, home)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn overview_lists_largest_and_suggestions() {
        let f = fixture();
        let screen = render(&f.app(), &f.home);
        assert!(screen.contains("510 GB used of 1.0 TB (51%)"), "{screen}");
        assert!(screen.contains("Largest folders"), "{screen}");
        assert!(screen.contains("~/old"), "{screen}");
        assert!(screen.contains("Could free ~"), "{screen}");
        assert!(screen.contains("once the Trash is emptied"), "{screen}");
    }

    #[test]
    fn browse_shows_other_files_row() {
        let f = fixture();
        let mut app = f.app();
        app.view = crate::tui::app::View::Browse(f.home.join("old"));
        app.reload(&f.store, &f.ctx, &f.probe).unwrap();
        let screen = render(&app, &f.home);
        assert!(screen.contains("1 other file"), "{screen}");
    }

    #[test]
    fn messages_keep_their_line_breaks() {
        let f = fixture();
        let mut app = f.app();
        app.view = crate::tui::app::View::Message("first line\n  ~/second".into());
        let screen = render(&app, &f.home);
        let rows: Vec<&str> = screen.lines().collect();
        let i = rows
            .iter()
            .position(|r| r.contains("first line"))
            .expect(&screen);
        assert!(rows[i + 1].contains("~/second"), "{screen}");
    }

    #[test]
    fn truncation_counts_display_width() {
        use unicode_width::UnicodeWidthStr;
        let t = truncate("/Users/me/写真/とても長いフォルダ名", 12);
        assert!(t.width() <= 12, "{t:?} is {} wide", t.width());
        assert!(t.starts_with('…'));
    }

    #[test]
    fn odd_names_are_drawn_on_one_line() {
        assert_eq!(
            display_path(
                std::path::Path::new("/h/a\nb\tc"),
                std::path::Path::new("/h")
            ),
            "~/a?b?c"
        );
        assert_eq!(
            display_path(
                std::path::Path::new("/elsewhere/x"),
                std::path::Path::new("/h")
            ),
            "/elsewhere/x"
        );
    }
}
