//! Times the git probe (`repo_has_unsaved_work`) on synthetic repos.
//!
//! `probe list` prints the scenarios, `probe build ROOT` writes the repos,
//! `probe run ROOT SCENARIO` probes one and prints `unsaved wall_ms cpu_ms`
//! (1 when the probe warns). The answer is not compared between versions:
//! a newer probe warns in cases an older one didn't.

use std::path::Path;

use attic_bench::{Scenario, add_history, git, measure, print_list, repo, write_files};
use attic_core::probe::{Probe, SystemProbe};

const fn s(kind: &'static str, name: &'static str, what: &'static str) -> Scenario {
    Scenario { kind, name, what }
}

const SCENARIOS: &[Scenario] = &[
    s(
        "typical",
        "project-clean",
        "project, 2k files, 10 branches, clean",
    ),
    s(
        "typical",
        "project-one-untracked",
        "same project, one untracked file",
    ),
    s(
        "typical",
        "node-modules-in-repo",
        "ignored node_modules (10k files) inside a repo",
    ),
    s(
        "typical",
        "five-projects",
        "folder holding 5 clean projects",
    ),
    s(
        "typical",
        "plain-folder",
        "plain folder, 500 files, no repo",
    ),
    s("stress", "big-clean", "clean repo, 20k files"),
    s("stress", "big-dirty", "dirty repo, 20k files"),
    s(
        "stress",
        "branches-300",
        "clean repo, 300 branches, 1000 commits",
    ),
    s(
        "stress",
        "branches-1000",
        "clean repo, 1000 branches, 300 commits",
    ),
    s("stress", "forty-repos", "folder holding 40 clean repos"),
    s("stress", "no-repo-100k", "100k files, no repo"),
    s(
        "stress",
        "untracked-in-repo",
        "folder inside a repo, 20k untracked files",
    ),
    s(
        "stress",
        "tracked-in-repo",
        "clean tracked folder inside a repo",
    ),
];

/// The folder a scenario probes, relative to the root.
fn target(name: &str) -> Option<&'static str> {
    Some(match name {
        "project-clean" => "project",
        "project-one-untracked" => "project-dirty",
        "node-modules-in-repo" => "project/node_modules",
        "five-projects" => "code",
        "plain-folder" => "downloads",
        "big-clean" => "big",
        "big-dirty" => "big-dirty",
        "branches-300" => "branches-300",
        "branches-1000" => "branches-1000",
        "forty-repos" => "many",
        "no-repo-100k" => "plain",
        "untracked-in-repo" => "big-dirty/data",
        "tracked-in-repo" => "big/d7",
        _ => return None,
    })
}

/// An ordinary project: 2k files, 500 commits, 10 branches, all pushed.
fn project(root: &Path, name: &str) -> std::path::PathBuf {
    let work = repo(root, name, 40, 50);
    add_history(&work, 500, 9);
    work
}

fn build(root: &Path) {
    let work = project(root, "project");
    std::fs::write(work.join(".gitignore"), "node_modules/\n").expect("write");
    git(&work, &["add", ".gitignore"]);
    git(&work, &["commit", "-q", "-m", "ignore"]);
    git(&work, &["push", "-q"]);
    write_files(&work.join("node_modules"), 500, 20);

    let dirty = project(root, "project-dirty");
    std::fs::write(dirty.join("d1/new.txt"), "untracked").expect("write");

    let code = root.join("code");
    std::fs::create_dir_all(&code).expect("create folder");
    for i in 0..5 {
        repo(&code, &format!("p{i}"), 20, 50);
    }
    write_files(&root.join("downloads"), 5, 100);

    repo(root, "big", 200, 100);
    let big_dirty = repo(root, "big-dirty", 200, 100);
    write_files(&big_dirty.join("data"), 200, 100);

    let many_branches = repo(root, "branches-300", 10, 10);
    add_history(&many_branches, 1000, 300);
    let more_branches = repo(root, "branches-1000", 10, 10);
    add_history(&more_branches, 300, 1000);

    let many = root.join("many");
    std::fs::create_dir_all(&many).expect("create folder");
    for i in 0..40 {
        repo(&many, &format!("r{i}"), 10, 50);
    }
    write_files(&root.join("plain"), 1000, 100);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => print_list(SCENARIOS),
        Some("build") => build(Path::new(&args[2])),
        Some("run") => {
            let path = Path::new(&args[2]).join(target(&args[3]).expect("known scenario"));
            measure(|| u8::from(SystemProbe.repo_has_unsaved_work(&path)));
        }
        _ => eprintln!("usage: probe list | build ROOT | run ROOT SCENARIO"),
    }
}
