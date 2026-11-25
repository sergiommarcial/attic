//! Questions about a path asked only for the suggestions shown: when it
//! was last used, and whether it holds a git repository with unsaved work.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// How deep below a suggestion to look for git repositories.
const REPO_SEARCH_DEPTH: usize = 3;

/// Facts about a path that are too slow to gather for every scanned file.
pub trait Probe {
    /// When the path was last used, as unix seconds, if anything knows.
    fn last_used(&self, path: &Path) -> Option<i64>;
    /// Whether a git repository at or up to three levels below `path` has
    /// uncommitted changes, a stash, or commits on any local branch that no
    /// remote has, or `path` sits inside a repository and has changed or
    /// untracked files there.
    fn repo_has_unsaved_work(&self, path: &Path) -> bool;
}

/// Asks Spotlight (`mdls`) and `git`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn last_used(&self, path: &Path) -> Option<i64> {
        let out = Command::new("mdls")
            .args(["-raw", "-name", "kMDItemLastUsedDate"])
            .arg(path)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| parse_mdls(&String::from_utf8_lossy(&out.stdout)))
            .flatten()
    }

    fn repo_has_unsaved_work(&self, path: &Path) -> bool {
        let mut repos = Vec::new();
        find_repos(path, REPO_SEARCH_DEPTH, &mut repos);
        repos.iter().any(|repo| unsaved(repo))
            || enclosing_repo(path).is_some_and(|repo| path_unsaved(repo, path))
    }
}

/// Answers nothing; for callers that only need sizes and ranks.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProbe;

impl Probe for NoProbe {
    fn last_used(&self, _: &Path) -> Option<i64> {
        None
    }

    fn repo_has_unsaved_work(&self, _: &Path) -> bool {
        false
    }
}

/// Remembers another probe's answers for `ttl`, so redrawing the same
/// suggestions doesn't spawn `mdls` and `git` again.
pub struct CachedProbe<P> {
    inner: P,
    ttl: Duration,
    last_used: RefCell<HashMap<PathBuf, (Instant, Option<i64>)>>,
    unsaved: RefCell<HashMap<PathBuf, (Instant, bool)>>,
}

impl<P: Probe> CachedProbe<P> {
    /// Caches `inner`'s answers for `ttl`.
    pub fn new(inner: P, ttl: Duration) -> Self {
        CachedProbe {
            inner,
            ttl,
            last_used: RefCell::default(),
            unsaved: RefCell::default(),
        }
    }

    /// The wrapped probe.
    pub fn inner(&self) -> &P {
        &self.inner
    }
}

fn cached<T: Copy>(
    cache: &RefCell<HashMap<PathBuf, (Instant, T)>>,
    ttl: Duration,
    path: &Path,
    ask: impl FnOnce() -> T,
) -> T {
    if let Some((at, value)) = cache.borrow().get(path)
        && at.elapsed() < ttl
    {
        return *value;
    }
    let value = ask();
    cache
        .borrow_mut()
        .insert(path.to_path_buf(), (Instant::now(), value));
    value
}

impl<P: Probe> Probe for CachedProbe<P> {
    fn last_used(&self, path: &Path) -> Option<i64> {
        cached(&self.last_used, self.ttl, path, || {
            self.inner.last_used(path)
        })
    }

    fn repo_has_unsaved_work(&self, path: &Path) -> bool {
        cached(&self.unsaved, self.ttl, path, || {
            self.inner.repo_has_unsaved_work(path)
        })
    }
}

/// A probe with canned answers, for tests.
#[derive(Debug, Clone, Default)]
pub struct FakeProbe {
    /// `last_used` answers.
    pub last_used: HashMap<PathBuf, i64>,
    /// Paths for which `repo_has_unsaved_work` is true.
    pub unsaved: HashSet<PathBuf>,
}

impl Probe for FakeProbe {
    fn last_used(&self, path: &Path) -> Option<i64> {
        self.last_used.get(path).copied()
    }

    fn repo_has_unsaved_work(&self, path: &Path) -> bool {
        self.unsaved.contains(path)
    }
}

fn find_repos(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        out.push(dir.to_path_buf());
    }
    if depth == 0 {
        return;
    }
    let Ok(listing) = std::fs::read_dir(dir) else {
        return;
    };
    for child in listing.flatten() {
        let is_dir = child.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && child.file_name() != ".git" {
            find_repos(&child.path(), depth - 1, out);
        }
    }
}

/// Runs `git` in `repo`. No optional locks: `status` must not rewrite
/// `.git/index` (which would make the repo look used). No fsmonitor: a
/// repo's own config must not get to run a program just because attic
/// looked at it. Literal pathspecs: a folder called `keep*` is that folder,
/// not a pattern.
fn git(repo: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["--literal-pathspecs", "-c", "core.fsmonitor=false", "-C"])
        .arg(repo)
        .args(args)
        .output()
        .ok()
}

/// Whether `git args` prints something. Unknown (git failed) counts as yes:
/// a warning that is wrong costs a keypress, a missing one can cost work.
fn prints(repo: &Path, args: &[&str]) -> bool {
    git(repo, args).is_none_or(|o| !o.status.success() || !o.stdout.is_empty())
}

/// The nearest repository above `path`, if `path` sits inside one.
fn enclosing_repo(path: &Path) -> Option<&Path> {
    path.ancestors()
        .skip(1)
        .find(|dir| dir.join(".git").exists())
}

/// Whether anything under `path` is changed or untracked (and not ignored)
/// in the repository around it. Stashes and unpushed branches are about the
/// whole repo and say nothing about what is lost by moving one folder.
fn path_unsaved(repo: &Path, path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return true;
    };
    prints(repo, &["status", "--porcelain", "--", path])
}

fn unsaved(repo: &Path) -> bool {
    // Changes, stashes (invisible to `status`), and commits on any local
    // branch that no remote has.
    if prints(repo, &["status", "--porcelain"])
        || prints(repo, &["stash", "list"])
        || prints(
            repo,
            &[
                "log",
                "--branches",
                "--not",
                "--remotes",
                "-1",
                "--format=%H",
            ],
        )
    {
        return true;
    }
    match git(repo, &["rev-list", "--count", "@{u}..HEAD"]) {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() != "0",
        _ => {
            git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).is_some_and(|o| o.status.success())
        }
    }
}

/// Parses `mdls -raw` output such as `2026-10-30 14:22:05 +0000`.
pub fn parse_mdls(text: &str) -> Option<i64> {
    let mut parts = text.split_whitespace();
    let (date, time, zone) = (parts.next()?, parts.next()?, parts.next()?);
    let mut d = date.split('-').map(|n| n.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let zone: i64 = zone.get(1..)?.parse().ok()?;
    let offset = sign * ((zone / 100) * 3600 + (zone % 100) * 60);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + h * 3600 + min * 60 + s - offset)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    #[test]
    fn parses_mdls_date() {
        assert_eq!(
            parse_mdls("2026-10-30 14:22:05 +0000\n"),
            Some(1_793_370_125)
        );
        assert_eq!(parse_mdls("1970-01-01 00:00:00 +0000"), Some(0));
    }

    #[test]
    fn mdls_null_and_junk_are_none() {
        assert_eq!(parse_mdls("(null)"), None);
        assert_eq!(parse_mdls(""), None);
        assert_eq!(parse_mdls("yesterday"), None);
    }

    #[test]
    fn last_used_of_a_fresh_file_does_not_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.txt");
        std::fs::write(&file, b"x").unwrap();
        // Spotlight may or may not know a temp file; it must not error.
        let _ = SystemProbe.last_used(&file);
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(["-c", "init.defaultBranch=main"])
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A clone of a bare remote with one pushed commit.
    fn synced_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let remote = dir.path().join("remote.git");
        let work = dir.path().join("projects/work");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "-q", "--bare"]);
        git(
            dir.path(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        std::fs::write(work.join("a.txt"), b"a").unwrap();
        git(&work, &["add", "a.txt"]);
        git(&work, &["commit", "-q", "-m", "a"]);
        git(&work, &["push", "-q", "-u", "origin", "HEAD"]);
        (dir, work)
    }

    #[test]
    fn clean_pushed_repo_is_saved() {
        let (dir, _work) = synced_repo();
        assert!(!SystemProbe.repo_has_unsaved_work(&dir.path().join("projects")));
    }

    #[test]
    fn dirty_repo_is_unsaved() {
        let (dir, work) = synced_repo();
        std::fs::write(work.join("b.txt"), b"b").unwrap();
        assert!(SystemProbe.repo_has_unsaved_work(&dir.path().join("projects")));
    }

    #[test]
    fn unpushed_commit_is_unsaved() {
        let (_dir, work) = synced_repo();
        std::fs::write(work.join("b.txt"), b"b").unwrap();
        git(&work, &["add", "b.txt"]);
        git(&work, &["commit", "-q", "-m", "b"]);
        assert!(SystemProbe.repo_has_unsaved_work(&work));
    }

    /// A commit on a new local branch, then back on `main`.
    fn commit_on_other_branch(work: &Path, branch: &str) {
        git(work, &["checkout", "-q", "-b", branch]);
        std::fs::write(work.join("c.txt"), b"c").unwrap();
        git(work, &["add", "c.txt"]);
        git(work, &["commit", "-q", "-m", "c"]);
        git(work, &["checkout", "-q", "main"]);
    }

    #[test]
    fn unpushed_commit_on_another_branch_is_unsaved() {
        let (_dir, work) = synced_repo();
        commit_on_other_branch(&work, "feature");
        assert!(SystemProbe.repo_has_unsaved_work(&work));
    }

    #[test]
    fn pushed_other_branch_is_saved() {
        let (_dir, work) = synced_repo();
        commit_on_other_branch(&work, "feature");
        git(&work, &["push", "-q", "-u", "origin", "feature"]);
        assert!(!SystemProbe.repo_has_unsaved_work(&work));
    }

    #[test]
    fn stash_is_unsaved_until_dropped() {
        let (_dir, work) = synced_repo();
        std::fs::write(work.join("a.txt"), b"changed").unwrap();
        git(&work, &["stash", "push", "-q"]);
        assert!(SystemProbe.repo_has_unsaved_work(&work));
        git(&work, &["stash", "drop", "-q"]);
        assert!(!SystemProbe.repo_has_unsaved_work(&work));
    }

    /// Commits and pushes `files` (paths relative to `work`).
    fn push_files(work: &Path, files: &[(&str, &str)]) {
        for (name, body) in files {
            let file = work.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, body).unwrap();
            git(work, &["add", name]);
        }
        git(work, &["commit", "-q", "-m", "files"]);
        git(work, &["push", "-q"]);
    }

    #[test]
    fn untracked_folder_inside_a_repo_is_unsaved() {
        let (_dir, work) = synced_repo();
        std::fs::create_dir_all(work.join("data")).unwrap();
        std::fs::write(work.join("data/f.txt"), b"f").unwrap();
        assert!(SystemProbe.repo_has_unsaved_work(&work.join("data")));
    }

    #[test]
    fn folder_name_with_glob_characters_is_matched_literally() {
        let (_dir, work) = synced_repo();
        let folder = work.join("Footage [2023]*");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("f.txt"), b"f").unwrap();
        assert!(SystemProbe.repo_has_unsaved_work(&folder));
    }

    #[test]
    fn glob_characters_do_not_match_siblings() {
        let (_dir, work) = synced_repo();
        push_files(&work, &[("keep*/f.txt", "f")]);
        std::fs::create_dir_all(work.join("keep2")).unwrap();
        std::fs::write(work.join("keep2/x.txt"), b"x").unwrap();
        assert!(!SystemProbe.repo_has_unsaved_work(&work.join("keep*")));
    }

    #[test]
    fn modified_tracked_file_inside_a_repo_is_unsaved() {
        let (_dir, work) = synced_repo();
        std::fs::write(work.join("a.txt"), b"changed").unwrap();
        assert!(SystemProbe.repo_has_unsaved_work(&work.join("a.txt")));
    }

    #[test]
    fn clean_tracked_folder_inside_a_repo_is_saved() {
        let (_dir, work) = synced_repo();
        push_files(&work, &[("docs/d.txt", "d")]);
        assert!(!SystemProbe.repo_has_unsaved_work(&work.join("docs")));
    }

    #[test]
    fn changes_elsewhere_in_the_repo_do_not_warn() {
        let (_dir, work) = synced_repo();
        push_files(&work, &[("docs/d.txt", "d")]);
        std::fs::create_dir_all(work.join("other")).unwrap();
        std::fs::write(work.join("other/x.txt"), b"x").unwrap();
        assert!(!SystemProbe.repo_has_unsaved_work(&work.join("docs")));
    }

    #[test]
    fn repo_wide_state_does_not_warn_for_a_folder_inside() {
        let (_dir, work) = synced_repo();
        push_files(&work, &[("docs/d.txt", "d")]);
        commit_on_other_branch(&work, "feature");
        std::fs::write(work.join("a.txt"), b"changed").unwrap();
        git(&work, &["stash", "push", "-q"]);
        assert!(!SystemProbe.repo_has_unsaved_work(&work.join("docs")));
    }

    #[test]
    fn ignored_folder_inside_a_repo_is_saved() {
        let (_dir, work) = synced_repo();
        push_files(&work, &[(".gitignore", "node_modules/\n")]);
        std::fs::create_dir_all(work.join("node_modules")).unwrap();
        std::fs::write(work.join("node_modules/f.js"), b"f").unwrap();
        assert!(!SystemProbe.repo_has_unsaved_work(&work.join("node_modules")));
    }

    #[test]
    fn empty_repo_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        assert!(!SystemProbe.repo_has_unsaved_work(dir.path()));
    }

    #[test]
    fn repo_without_upstream_with_commits_is_unsaved() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "a"]);
        assert!(SystemProbe.repo_has_unsaved_work(dir.path()));
    }

    #[test]
    fn no_repo_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        assert!(!SystemProbe.repo_has_unsaved_work(dir.path()));
    }

    #[test]
    fn git_probe_runs_no_repo_hooks_and_writes_nothing() {
        let (dir, work) = synced_repo();
        let marker = dir.path().join("fsmonitor-ran");
        let hook = dir.path().join("hook.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        git(&work, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
        // A changed mtime makes git want to refresh (and write) the index.
        std::fs::write(work.join("a.txt"), b"a").unwrap();
        let index = std::fs::metadata(work.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let _ = SystemProbe.repo_has_unsaved_work(&work);
        assert!(!marker.exists(), "core.fsmonitor ran");
        let after = std::fs::metadata(work.join(".git/index"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(index, after, ".git/index was rewritten");
    }

    #[test]
    fn cached_probe_asks_once_per_path() {
        struct Counting(std::cell::Cell<u32>);
        impl Probe for Counting {
            fn last_used(&self, _: &Path) -> Option<i64> {
                self.0.set(self.0.get() + 1);
                Some(5)
            }
            fn repo_has_unsaved_work(&self, _: &Path) -> bool {
                self.0.set(self.0.get() + 1);
                true
            }
        }
        let cached = CachedProbe::new(
            Counting(std::cell::Cell::new(0)),
            std::time::Duration::from_secs(60),
        );
        for _ in 0..3 {
            assert_eq!(cached.last_used(Path::new("/a")), Some(5));
            assert!(cached.repo_has_unsaved_work(Path::new("/a")));
        }
        assert_eq!(cached.inner().0.get(), 2);
        let expiring =
            CachedProbe::new(Counting(std::cell::Cell::new(0)), std::time::Duration::ZERO);
        expiring.last_used(Path::new("/a"));
        expiring.last_used(Path::new("/a"));
        assert_eq!(expiring.inner().0.get(), 2);
    }

    #[test]
    fn repos_deeper_than_three_levels_are_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("1/2/3/4");
        std::fs::create_dir_all(&deep).unwrap();
        git(&deep, &["init", "-q"]);
        std::fs::write(deep.join("a.txt"), b"a").unwrap();
        assert!(!SystemProbe.repo_has_unsaved_work(dir.path()));
    }
}
