//! `attic`: an offline disk explorer for macOS.

mod commands;
mod launchd;
mod tui;

use std::process::ExitCode;

use attic_core::Paths;
use clap::{Parser, Subcommand};

/// See where your disk space went and clean it up.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the background service (launchd starts this; you rarely need to).
    Daemon,
    /// Scan the disk once, in the foreground.
    Scan,
    /// Show whether the service runs, the last scan, and unreadable folders.
    Status,
    /// Start the service at login.
    Install,
    /// Stop the service and remove it from login.
    Uninstall,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = Paths::from_env().and_then(|paths| match cli.command {
        None => tui::run(&paths),
        Some(Command::Install) => launchd::install(&paths),
        Some(Command::Uninstall) => launchd::uninstall(&paths),
        Some(Command::Daemon) => commands::daemon(&paths),
        Some(Command::Scan) => commands::scan(&paths),
        Some(Command::Status) => commands::status(&paths),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("attic: {e}");
            ExitCode::FAILURE
        }
    }
}
