//! Pieces shared by the benchmark programs: synthetic trees, git repos and
//! timing. Nothing here touches the real disk; everything lives under a
//! folder the caller passes in.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// One thing to measure: `kind` is "typical" or "stress", `name` is what
/// `run` takes, `what` is the row label in the comparison table.
pub struct Scenario {
    pub kind: &'static str,
    pub name: &'static str,
    pub what: &'static str,
}

/// User and system CPU time of this process so far, in seconds.
pub fn cpu_seconds() -> f64 {
    // SAFETY: `rusage` is plain data and `getrusage` fills it in.
    let usage = unsafe {
        let mut usage: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut usage);
        usage
    };
    let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    secs(usage.ru_utime) + secs(usage.ru_stime)
}

/// Runs `f` once and prints `answer wall_ms cpu_ms`, the line
/// `benches/compare.py` reads. `answer` lets the driver check that two
/// versions agree where they should (row counts for a scan).
pub fn measure<T: std::fmt::Display>(f: impl FnOnce() -> T) {
    let (cpu, start) = (cpu_seconds(), Instant::now());
    let answer = f();
    let wall = start.elapsed().as_secs_f64() * 1e3;
    println!("{answer} {wall:.2} {:.2}", (cpu_seconds() - cpu) * 1e3);
}

/// Runs git in `dir` with a fixed identity, so repos are reproducible.
///
/// # Panics
///
/// When git fails: a benchmark with a half-built tree measures nothing.
pub fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@example.com",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs")
        .status
        .success();
    assert!(ok, "git {args:?} failed in {}", dir.display());
}

/// Writes `dirs` folders of `per` small files each under `dir`.
pub fn write_files(dir: &Path, dirs: usize, per: usize) {
    for d in 0..dirs {
        let folder = dir.join(format!("d{d}"));
        std::fs::create_dir_all(&folder).expect("create folder");
        for f in 0..per {
            std::fs::write(folder.join(format!("f{f}.txt")), format!("{d} {f}")).expect("write");
        }
    }
}

/// A clone of a bare remote under `root`, holding `dirs` x `per` files in
/// one pushed commit.
pub fn repo(root: &Path, name: &str, dirs: usize, per: usize) -> PathBuf {
    let remote = root.join(format!("{name}.git"));
    let work = root.join(name);
    std::fs::create_dir_all(&remote).expect("create remote");
    git(&remote, &["init", "-q", "--bare"]);
    git(
        root,
        &[
            "clone",
            "-q",
            remote.to_str().expect("utf-8"),
            work.to_str().expect("utf-8"),
        ],
    );
    write_files(&work, dirs, per);
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "files"]);
    git(&work, &["push", "-q", "-u", "origin", "HEAD"]);
    work
}

/// Adds `commits` empty commits and `branches` branches to `work`, then
/// pushes everything.
pub fn add_history(work: &Path, commits: usize, branches: usize) {
    for i in 0..commits {
        git(
            work,
            &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")],
        );
    }
    for i in 0..branches {
        git(work, &["branch", &format!("b{i}")]);
    }
    git(work, &["push", "-q", "--all", "origin"]);
}

/// Prints the scenarios as `kind<TAB>name<TAB>what` lines.
pub fn print_list(scenarios: &[Scenario]) {
    for s in scenarios {
        println!("{}\t{}\t{}", s.kind, s.name, s.what);
    }
}
