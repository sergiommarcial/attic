# attic

[![ci](https://github.com/sergiommarcial/attic/actions/workflows/ci.yml/badge.svg)](https://github.com/sergiommarcial/attic/actions/workflows/ci.yml)

attic shows where your Mac's disk space went and helps you get it back. It
lists the largest and least recently used folders and files, suggests what
to clean, and moves what you pick to the Trash. Everything stays on your
machine: attic makes no network requests.

It has two parts in one binary:

- a background service (`attic daemon`) that launchd starts at login. It
  scans your home folder once, then follows macOS file system events to
  stay current, and sends a notification when there is something worth
  cleaning.
- a terminal app (`attic`) that reads what the service found, so it opens
  instantly.

macOS only.

## Install

With Homebrew:

```sh
brew tap sergiommarcial/attic
brew trust sergiommarcial/attic   # Homebrew asks you to trust third-party taps
brew install sergiommarcial/attic/attic
brew services start attic         # the background service, now and at login
```

Homebrew builds attic from source, so the first install takes a few minutes.
With a Homebrew install, `attic install` refuses and points to
`brew services`, so the service never runs twice.

From a clone of this repo:

```sh
make install          # builds and installs attic into ~/.cargo/bin
attic install         # starts the background service, now and at login
```

Then give the binary Full Disk Access (System Settings, Privacy & Security,
Full Disk Access): `~/.cargo/bin/attic`, or with Homebrew the path `attic
status` shows. Grant it again after each `brew upgrade attic`. Without it,
macOS hides some folders (Mail, Messages, parts of `~/Library`); attic lists
the ones it could not read in `attic status` and in the app (press `u`), and
leaves them out of the totals. After granting access, restart the service
(`attic install` again, or `brew services restart attic`): it rereads those
folders. Rebuilding the binary can drop the grant, so re-check it after
`make install`.

The first scan of a large home folder takes a few minutes. The app shows its
progress.

## Commands

| Command | What it does |
|---|---|
| `attic` | The terminal app. |
| `attic status` | Whether the service runs, when the last full scan finished, and which folders could not be read. |
| `attic scan` | A full scan in the foreground, for use without the service (it refuses while the service runs). |
| `attic install` / `attic uninstall` | Start the service at login, or stop and remove it. |
| `attic daemon` | The service itself; launchd runs it. |

In the app: `↑`/`↓` (or `j`/`k`) move, `Tab` switches between the two lists,
`Enter` opens a folder, `Backspace` goes back, `s` sorts by size or by least
recently used, `c` cleans the selected item, `u` lists the folders attic
could not read, `q` quits.

## What it suggests

Every folder and every file of at least 500 MB is a candidate. They are
ranked by size, weighted by how long since you last used them (Spotlight's
"last used" date, or the newest change inside when Spotlight doesn't know),
so a big folder nobody has opened in months comes first. Your home folder,
the scan roots and the standard home folders (`~/Documents`, `~/Library` and
so on) are never suggested or cleaned whole, though things inside them are.
When most of a folder's size sits in one subfolder, the subfolder is
suggested instead, and a folder you used this week gives way to the old
things inside it. The Trash and attic's own folder are never scanned.

Some suggestions carry a `!`: a git repository with uncommitted or unpushed
work, a macOS library (Photos, Mail, Messages), or something used in the last
week. Cleaning one of those asks twice. Build output and caches that their
tools rebuild (`target/` folders, `node_modules`, `.venv`, `~/Library/Caches`)
are tagged `rebuildable`.

"Could free" is the total of the suggestions shown. Moving things to the
Trash frees nothing until the Trash is emptied, and Time Machine's local
snapshots can hold deleted data for a while, so the number is an estimate.

attic only ever moves things to the Trash, through Finder, so Finder's "Put
Back" works. The first time, macOS may ask whether attic can control Finder.
The background service never deletes anything.

## Notifications

The service sends at most one notification a day, once the space you could
free reaches 5 GB and has grown by 5 GB since the last notification (or since
you last cleaned up). Clicking it does nothing yet; run `attic` to review.

The service waits for a changing folder to go quiet before it reads it again,
so sizes can trail the disk by about 5 seconds, and by up to a minute for a
folder that never stops changing (a build in progress, say).

## Config

`~/.config/attic/config.toml` is optional. Every key has a default:

```toml
roots = []                  # folders to scan besides your home folder, e.g. ["/Volumes/Data"]
excludes = []               # folders never scanned or suggested, e.g. ["~/Music/Logic"]
min_file_size = "500MB"     # the smallest file that is listed and suggested on its own
notify_threshold = "5GB"    # how much could be freed before a notification
notify_max_per_day = 1      # 0 turns notifications off
```

Sizes take `kB`, `MB`, `GB` or `TB` (decimal, as Finder counts) or a plain
number of bytes. The service picks up changes on its own: new roots are
scanned, removed roots and newly excluded folders dropped.

## Files

- `~/Library/Application Support/attic/attic.db`: what the scans found.
  Deleting it makes the next start rescan.
- `~/Library/LaunchAgents/local.attic.daemon.plist`: the service's launchd
  entry, written by `attic install` (Homebrew's is `sh.brew.attic.plist`,
  or `homebrew.mxcl.attic.plist` before Homebrew 7, written by `brew
  services`).
- `~/Library/Logs/attic/`: the service's output (Homebrew:
  `$(brew --prefix)/var/log/attic.log`). A rescan of a folder that takes
  0.2 seconds or more is logged with its path, and so is a full rescan after
  macOS lost file system events, so a folder that keeps the service busy
  shows up there.

## Uninstall

With Homebrew: `brew services stop attic`, `brew uninstall attic`,
`brew untap sergiommarcial/attic`, then remove the data folders below and
`$(brew --prefix)/var/log/attic.log`. Otherwise:

```sh
attic uninstall
rm -rf ~/Library/Application\ Support/attic ~/Library/Logs/attic ~/.config/attic
cargo uninstall attic
```

## Development

`make help` lists the targets; `make ci` runs formatting, clippy, the tests,
the release script tests, actionlint, shellcheck, cargo-deny and
cargo-machete (`make install-tools` installs the tools). GitHub Actions runs
the same checks on every push and pull request. Tests never touch your real
home folder or Trash. Design decisions are in `docs/adr/`.

### Releasing

Bump `version` in `Cargo.toml` and run `cargo check` so `Cargo.lock` follows,
commit both and push to `main`, then push a tag `vX.Y.Z` matching it. The
release workflow checks the tag, runs the checks, creates the GitHub release
and updates the formula in
[homebrew-attic](https://github.com/sergiommarcial/homebrew-attic).

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request. To
report a vulnerability, follow [SECURITY.md](SECURITY.md) instead of opening
a public issue.

## License

MIT, see [LICENSE](LICENSE).
