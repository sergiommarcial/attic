//! Times `scan_subtree` on synthetic trees.
//!
//! `scan list` prints the scenarios, `scan build ROOT` writes the trees,
//! `scan run ROOT SCENARIO` scans one and prints `rows wall_ms cpu_ms`.

use std::path::Path;

use attic_bench::{Scenario, measure, print_list, write_files};
use attic_core::Config;
use attic_core::scan::scan_subtree;

const DEFAULT_MIN: u64 = 500_000_000;

const SCENARIOS: &[Scenario] = &[
    Scenario {
        kind: "typical",
        name: "project",
        what: "project, 3k files",
    },
    Scenario {
        kind: "typical",
        name: "home",
        what: "home-like tree, 146k files",
    },
    Scenario {
        kind: "stress",
        name: "home-all",
        what: "home-like tree, every file a row",
    },
    Scenario {
        kind: "stress",
        name: "nested",
        what: "400 folders, 8k files",
    },
    Scenario {
        kind: "stress",
        name: "wide",
        what: "one folder, 100k files",
    },
];

/// Folder, and the smallest file that gets its own row, for a scenario.
fn setup(name: &str) -> Option<(&'static str, u64)> {
    Some(match name {
        "project" => ("project", DEFAULT_MIN),
        "home" => ("home", DEFAULT_MIN),
        "home-all" => ("home", 1),
        "nested" => ("nested", DEFAULT_MIN),
        "wide" => ("wide", DEFAULT_MIN),
        _ => return None,
    })
}

/// `top` x `mid` folders, each with `low` leaf folders of `per_leaf` files
/// and `per_mid` files of its own.
fn tree(root: &Path, shape: [usize; 5]) {
    let [top, mid, low, per_leaf, per_mid] = shape;
    for a in 0..top {
        for b in 0..mid {
            let folder = root.join(format!("top{a}/mid{b}"));
            std::fs::create_dir_all(&folder).expect("create folder");
            for i in 0..per_mid {
                std::fs::write(folder.join(format!("f{i}.dat")), [7u8; 200]).expect("write");
            }
            for c in 0..low {
                let leaf = folder.join(format!("leaf{c}"));
                std::fs::create_dir(&leaf).expect("create leaf");
                for i in 0..per_leaf {
                    std::fs::write(leaf.join(format!("f{i}.dat")), [7u8; 200]).expect("write");
                }
            }
        }
    }
}

fn build(root: &Path) {
    tree(&root.join("project"), [4, 5, 8, 10, 3]);
    tree(&root.join("home"), [20, 20, 20, 18, 5]);
    tree(&root.join("nested"), [1, 1, 400, 20, 0]);
    let wide = root.join("wide");
    std::fs::create_dir_all(&wide).expect("create folder");
    write_files(&wide, 1, 100_000);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("list") => print_list(SCENARIOS),
        Some("build") => build(Path::new(&args[2])),
        Some("run") => {
            let (folder, min_file_size) = setup(&args[3]).expect("known scenario");
            let cfg = Config {
                min_file_size,
                ..Config::default()
            };
            let path = Path::new(&args[2]).join(folder);
            measure(|| scan_subtree(&path, &cfg).expect("scan").len());
        }
        _ => eprintln!("usage: scan list | build ROOT | run ROOT SCENARIO"),
    }
}
