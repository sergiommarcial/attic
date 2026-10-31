//! When to tell the user there is something to clean, and how.

use std::cell::RefCell;
use std::process::Command;

use crate::units::human;
use crate::{AtticError, Config, Result};

const DAY: i64 = 86_400;

/// Whether a notification is due: the total that could be freed reaches
/// the threshold, has grown by at least the threshold since the last one
/// (`last` = its total and time), and the daily cap allows it.
pub fn should_notify(total: u64, last: Option<(u64, i64)>, now: i64, cfg: &Config) -> bool {
    if cfg.notify_max_per_day == 0 || total < cfg.notify_threshold {
        return false;
    }
    match last {
        None => true,
        Some((last_total, at)) => {
            now - at >= DAY / i64::from(cfg.notify_max_per_day)
                && total >= last_total.saturating_add(cfg.notify_threshold)
        }
    }
}

/// The notification's body.
pub fn message(total: u64) -> String {
    format!(
        "About {} could be freed. Run attic to review.",
        human(total)
    )
}

/// Sends a notification.
pub trait Notifier {
    /// Shows `body` under `title`.
    ///
    /// # Errors
    ///
    /// [`AtticError::Command`] when it could not be shown.
    fn send(&self, title: &str, body: &str) -> Result<()>;
}

/// Uses `osascript`'s `display notification`. Clicking it does nothing,
/// since that needs a signed app bundle.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsascriptNotifier;

impl Notifier for OsascriptNotifier {
    fn send(&self, title: &str, body: &str) -> Result<()> {
        let fail = |message: String| AtticError::Command {
            program: "osascript".into(),
            message,
        };
        let out = Command::new("osascript")
            .arg("-e")
            .arg(applescript(title, body))
            .output()
            .map_err(|e| fail(e.to_string()))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(fail(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
}

/// Keeps what it was asked to send. For tests.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    sent: RefCell<Vec<(String, String)>>,
}

impl RecordingNotifier {
    /// Every `(title, body)` sent so far.
    pub fn sent(&self) -> Vec<(String, String)> {
        self.sent.borrow().clone()
    }
}

impl Notifier for RecordingNotifier {
    fn send(&self, title: &str, body: &str) -> Result<()> {
        self.sent.borrow_mut().push((title.into(), body.into()));
        Ok(())
    }
}

/// The AppleScript line for a notification, with `"` and `\` escaped.
pub fn applescript(title: &str, body: &str) -> String {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "display notification \"{}\" with title \"{}\"",
        quote(body),
        quote(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    const GB: u64 = 1_000_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn first_notification_at_threshold() {
        let cfg = Config::default();
        assert!(!should_notify(4_900_000_000, None, 0, &cfg));
        assert!(should_notify(5 * GB, None, 0, &cfg));
    }

    #[test]
    fn needs_growth_since_last() {
        let cfg = Config::default();
        let last = Some((10 * GB, 0));
        assert!(!should_notify(14 * GB, last, 2 * DAY, &cfg));
        assert!(should_notify(15 * GB, last, 2 * DAY, &cfg));
    }

    #[test]
    fn at_most_once_a_day() {
        let cfg = Config::default();
        let last = Some((5 * GB, 1_000));
        assert!(!should_notify(50 * GB, last, 1_000 + DAY - 1, &cfg));
        assert!(should_notify(50 * GB, last, 1_000 + DAY, &cfg));
        let twice = Config {
            notify_max_per_day: 2,
            ..Config::default()
        };
        assert!(should_notify(50 * GB, last, 1_000 + DAY / 2, &twice));
    }

    #[test]
    fn zero_per_day_never() {
        let cfg = Config {
            notify_max_per_day: 0,
            ..Config::default()
        };
        assert!(!should_notify(500 * GB, None, 0, &cfg));
    }

    #[test]
    fn message_text() {
        assert_eq!(
            message(18_200_000_000),
            "About 18 GB could be freed. Run attic to review."
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        assert_eq!(
            applescript("attic", r#"say "hi" \ bye"#),
            r#"display notification "say \"hi\" \\ bye" with title "attic""#
        );
    }
}
