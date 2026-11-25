//! The per-user launchd agent that runs `attic daemon`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use attic_core::{AtticError, Paths, Result};

use crate::commands::full_disk_access_hint;

/// The agent's launchd label.
pub const LABEL: &str = "local.attic.daemon";

/// The labels the service may run under: `attic install`'s, and the one
/// `brew services` gives it (`homebrew.mxcl.attic` before Homebrew 7,
/// `sh.brew.attic` since).
pub const SERVICE_LABELS: [&str; 3] = [LABEL, "homebrew.mxcl.attic", "sh.brew.attic"];

const BOOTSTRAP_ATTEMPTS: u32 = 5;
const BOOTSTRAP_DELAY: Duration = Duration::from_millis(500);

/// Calls `f` up to `attempts` times, waiting `delay` between failures, and
/// returns the first success or the last error.
pub fn retry<T, E>(
    attempts: u32,
    delay: Duration,
    mut f: impl FnMut() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    let mut left = attempts.max(1);
    loop {
        match f() {
            Ok(value) => return Ok(value),
            Err(e) if left <= 1 => return Err(e),
            Err(_) => {
                left -= 1;
                std::thread::sleep(delay);
            }
        }
    }
}

/// The agent definition: run `<exe> daemon` at login, at background
/// priority, logging under `log_dir`. It is restarted only after a failure,
/// and then at most every five minutes, so a persistent startup error (a
/// newer database, say) doesn't spin.
pub fn plist(exe: &Path, log_dir: &Path) -> String {
    let exe = xml(&exe.display().to_string());
    let out = xml(&log_dir.join("daemon.out.log").display().to_string());
    let err = xml(&log_dir.join("daemon.err.log").display().to_string());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
\t<key>Label</key>
\t<string>{LABEL}</string>
\t<key>ProgramArguments</key>
\t<array>
\t\t<string>{exe}</string>
\t\t<string>daemon</string>
\t</array>
\t<key>RunAtLoad</key>
\t<true/>
\t<key>KeepAlive</key>
\t<dict>
\t\t<key>SuccessfulExit</key>
\t\t<false/>
\t</dict>
\t<key>ThrottleInterval</key>
\t<integer>300</integer>
\t<key>ProcessType</key>
\t<string>Background</string>
\t<key>LowPriorityIO</key>
\t<true/>
\t<key>Nice</key>
\t<integer>10</integer>
\t<key>StandardOutPath</key>
\t<string>{out}</string>
\t<key>StandardErrorPath</key>
\t<string>{err}</string>
</dict>
</plist>
"
    )
}

/// Where the agent's plist goes.
pub fn plist_path(launch_agents: &Path) -> PathBuf {
    launch_agents.join(format!("{LABEL}.plist"))
}

/// Whether `exe` was installed by Homebrew (its real path is in a Cellar).
pub fn is_homebrew(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "Cellar")
}

/// A Homebrew install runs its service through `brew services`; a second
/// agent from `attic install` would run the service twice.
///
/// # Errors
///
/// [`AtticError::Command`] for a binary inside a Homebrew Cellar.
pub fn refuse_homebrew(exe: &Path) -> Result<()> {
    if is_homebrew(exe) {
        return Err(AtticError::Command {
            program: "install".into(),
            message: "attic was installed with Homebrew; start the service with `brew services start attic`".into(),
        });
    }
    Ok(())
}

/// Whether `exe` is a cargo build output, which a rebuild replaces (and
/// may lose its Full Disk Access grant with).
pub fn is_build_output(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "target")
}

/// Writes the plist for the running binary and (re)loads the agent.
///
/// # Errors
///
/// [`AtticError::Io`] when the plist cannot be written,
/// [`AtticError::Command`] when `launchctl` refuses it.
pub fn install(paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| AtticError::io("current executable", e))?;
    refuse_homebrew(&exe)?;
    if is_build_output(&exe) {
        eprintln!(
            "attic: warning: installing {} from a build folder; a rebuild may drop its Full Disk Access. `make install` first, then run the installed attic.",
            exe.display()
        );
    }
    for dir in [&paths.launch_agents, &paths.logs] {
        std::fs::create_dir_all(dir).map_err(|e| AtticError::io(dir, e))?;
    }
    let file = plist_path(&paths.launch_agents);
    std::fs::write(&file, plist(&exe, &paths.logs)).map_err(|e| AtticError::io(&file, e))?;
    let _ = launchctl(&["bootout", &service_target()]);
    // Right after a bootout launchd may still be tearing the old job down.
    retry(BOOTSTRAP_ATTEMPTS, BOOTSTRAP_DELAY, || {
        launchctl(&["bootstrap", &domain(), &file.display().to_string()])
    })?;
    println!("Installed {LABEL}; it starts now and at every login.");
    println!("{}", full_disk_access_hint());
    Ok(())
}

/// Unloads the agent and removes its plist.
///
/// # Errors
///
/// [`AtticError::Io`] when the plist exists but cannot be removed.
pub fn uninstall(paths: &Paths) -> Result<()> {
    let _ = launchctl(&["bootout", &service_target()]);
    let file = plist_path(&paths.launch_agents);
    match std::fs::remove_file(&file) {
        Ok(()) => println!("Removed {LABEL}."),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("{LABEL} was not installed.")
        }
        Err(e) => return Err(AtticError::io(&file, e)),
    }
    Ok(())
}

fn domain() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

fn service_target() -> String {
    format!("{}/{LABEL}", domain())
}

fn launchctl(args: &[&str]) -> Result<()> {
    let fail = |message: String| AtticError::Command {
        program: "launchctl".into(),
        message,
    };
    let out = Command::new("launchctl")
        .args(args)
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

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plist_has_every_key() {
        let p = plist(
            Path::new("/Users/me/.cargo/bin/attic"),
            Path::new("/Users/me/Library/Logs/attic"),
        );
        for needle in [
            "<key>Label</key>\n\t<string>local.attic.daemon</string>",
            "<string>/Users/me/.cargo/bin/attic</string>\n\t\t<string>daemon</string>",
            "<key>RunAtLoad</key>\n\t<true/>",
            "<key>ProcessType</key>\n\t<string>Background</string>",
            "<key>LowPriorityIO</key>\n\t<true/>",
            "<key>Nice</key>\n\t<integer>10</integer>",
            "<string>/Users/me/Library/Logs/attic/daemon.out.log</string>",
            "<string>/Users/me/Library/Logs/attic/daemon.err.log</string>",
        ] {
            assert!(p.contains(needle), "missing {needle:?} in\n{p}");
        }
        assert!(p.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
    }

    #[test]
    fn plist_restarts_only_after_a_failure_and_throttles() {
        let p = plist(Path::new("/x/attic"), Path::new("/l"));
        assert!(
            p.contains("<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>"),
            "{p}"
        );
        assert!(
            p.contains("<key>ThrottleInterval</key>\n\t<integer>300</integer>"),
            "{p}"
        );
    }

    #[test]
    fn retries_until_it_succeeds() {
        let mut calls = 0;
        let result = retry(5, std::time::Duration::ZERO, || {
            calls += 1;
            if calls < 3 { Err("busy") } else { Ok(calls) }
        });
        assert_eq!(result, Ok(3));
        let mut calls = 0;
        let result: std::result::Result<(), &str> = retry(4, std::time::Duration::ZERO, || {
            calls += 1;
            Err("no")
        });
        assert_eq!((result, calls), (Err("no"), 4));
    }

    #[test]
    fn plist_escapes_odd_exe_paths() {
        let p = plist(Path::new("/tmp/a&b<c>/attic"), Path::new("/l"));
        assert!(
            p.contains("<string>/tmp/a&amp;b&lt;c&gt;/attic</string>"),
            "{p}"
        );
    }

    #[test]
    fn plist_path_is_named_after_the_label() {
        assert_eq!(
            plist_path(Path::new("/h/Library/LaunchAgents")),
            Path::new("/h/Library/LaunchAgents/local.attic.daemon.plist")
        );
    }

    #[test]
    fn homebrew_installs_are_noticed() {
        assert!(is_homebrew(Path::new(
            "/opt/homebrew/Cellar/attic/0.1.0/bin/attic"
        )));
        assert!(is_homebrew(Path::new(
            "/usr/local/Cellar/attic/0.1.0/bin/attic"
        )));
        assert!(!is_homebrew(Path::new("/Users/me/.cargo/bin/attic")));
    }

    #[test]
    fn every_homebrew_service_name_counts_as_the_service() {
        assert_eq!(
            SERVICE_LABELS,
            ["local.attic.daemon", "homebrew.mxcl.attic", "sh.brew.attic"]
        );
    }

    #[test]
    fn install_points_homebrew_users_to_brew_services() {
        let err =
            refuse_homebrew(Path::new("/opt/homebrew/Cellar/attic/0.1.0/bin/attic")).unwrap_err();
        assert!(
            err.to_string().contains("brew services start attic"),
            "{err}"
        );
        assert!(refuse_homebrew(Path::new("/Users/me/.cargo/bin/attic")).is_ok());
    }

    #[test]
    fn target_dir_builds_are_noticed() {
        assert!(is_build_output(Path::new("/r/attic/target/debug/attic")));
        assert!(!is_build_output(Path::new("/Users/me/.cargo/bin/attic")));
    }
}
