#!/usr/bin/env python3
"""Measures attic's speed on synthetic trees, optionally against a git ref.

usage: compare.py [BASE_REF] [--runs N] [--only scan|probe]

With BASE_REF (say origin/main, or the commit before a change) the script
extracts that ref with `git archive`, builds the benchmark programs from
this checkout against both versions of attic-core, and runs them alternately
so drift hits both the same way. Without it, it measures the current code.
Each measurement is a fresh process (so the times include its start-up and a
cold CPU, a little higher than an in-process loop would show; ratios agree).
Trees are built once in a temp folder, never on the real disk, and removed
at the end. The output is a markdown table, ready for a PR or the README.

The base ref has to build with the current benchmark sources, so it can't be
older than the API they use (attic_core::scan and attic_core::probe).
"""
import argparse
import os
import pathlib
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parents[1]
PROGRAMS = ["scan", "probe"]
# Versions must agree on these answers (row counts); a newer probe may
# legitimately warn where an older one didn't.
SAME_ANSWER = {"scan": True, "probe": False}
# Below this a time is process noise and a ratio against it means nothing
# (an older version that did no work at all).
NOISE_FLOOR_MS = 0.2


def median(values):
    return statistics.median(values)


def p90(values):
    ordered = sorted(values)
    return ordered[int(len(ordered) * 0.9)]


def fmt_ms(ms):
    """Milliseconds as the README prints them: `0.4 ms`, `4.4 ms`, `329 ms`, `2.1 s`."""
    if ms >= 1000:
        return f"{ms / 1000:.1f} s"
    if ms >= 100:
        return f"{ms:.0f} ms"
    return f"{ms:.1f} ms"


def parse_run(line):
    """`answer wall_ms cpu_ms` as printed by a benchmark program."""
    answer, wall, cpu = line.split()
    return answer, float(wall), float(cpu)


def parse_list(text):
    """`kind<TAB>name<TAB>what` lines as (kind, name, what)."""
    return [tuple(line.split("\t", 2)) for line in text.splitlines() if line.strip()]


def label(kind, what):
    return f"**Typical:** {what}" if kind == "typical" else f"Stress: {what}"


def table(rows, base_name=None):
    """A markdown table. `rows` are dicts with kind, what, new and, when
    comparing, base (median milliseconds)."""
    if base_name:
        out = [f"| Scenario | {base_name} | new | change |", "|---|---|---|---|"]
    else:
        out = ["| Scenario | median |", "|---|---|"]
    for row in rows:
        if base_name:
            change = f"x{row['new'] / row['base']:.2f}" if row["base"] >= NOISE_FLOOR_MS else "n/a"
            out.append(
                f"| {label(row['kind'], row['what'])} | {fmt_ms(row['base'])} "
                f"| {fmt_ms(row['new'])} | {change} |"
            )
        else:
            out.append(f"| {label(row['kind'], row['what'])} | {fmt_ms(row['new'])} |")
    return "\n".join(out)


def run_cmd(args, **kwargs):
    return subprocess.run(args, check=True, capture_output=True, text=True, **kwargs).stdout


def build(source, target_dir):
    """Builds the benchmark programs in `source` and returns their paths."""
    env = {**os.environ, "CARGO_TARGET_DIR": str(target_dir)}
    subprocess.run(
        ["cargo", "build", "--release", "--locked", "--manifest-path", str(source / "benches/Cargo.toml")],
        check=True,
        env=env,
    )
    return {name: target_dir / "release" / name for name in PROGRAMS}


def extract(ref, dest):
    """The files of `ref`, plus this checkout's benches/ on top of them."""
    dest.mkdir(parents=True)
    archive = subprocess.Popen(["git", "-C", str(ROOT), "archive", ref], stdout=subprocess.PIPE)
    subprocess.run(["tar", "-x", "-C", str(dest)], stdin=archive.stdout, check=True)
    if archive.wait() != 0:
        sys.exit(f"git archive {ref} failed")
    shutil.copytree(
        ROOT / "benches", dest / "benches", dirs_exist_ok=True, ignore=shutil.ignore_patterns("target")
    )


def one(binary, trees, name):
    return parse_run(run_cmd([str(binary), "run", str(trees), name]).strip())


def compare_program(program, bins, trees, runs, only_new):
    rows = []
    scenarios = parse_list(run_cmd([str(bins["new"][program]), "list"]))
    for kind, name, what in scenarios:
        versions = ["new"] if only_new else ["base", "new"]
        for v in versions:  # warm the page cache and the process path
            one(bins[v][program], trees, name)
        walls = {v: [] for v in versions}
        answers = {v: set() for v in versions}
        for _ in range(runs):
            for v in versions:
                answer, wall, _cpu = one(bins[v][program], trees, name)
                walls[v].append(wall)
                answers[v].add(answer)
        if not only_new and SAME_ANSWER[program] and answers["base"] != answers["new"]:
            sys.exit(f"{program} {name}: the versions disagree: {answers}")
        row = {"kind": kind, "what": what, "new": median(walls["new"])}
        if not only_new:
            row["base"] = median(walls["base"])
        rows.append(row)
        print(f"  {program}/{name}: done", file=sys.stderr)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("base", nargs="?", help="git ref to compare against")
    parser.add_argument("--runs", type=int, default=15)
    parser.add_argument("--only", choices=PROGRAMS)
    args = parser.parse_args()
    programs = [args.only] if args.only else PROGRAMS
    started = time.time()

    work = pathlib.Path(tempfile.mkdtemp(prefix="attic-bench.")).resolve()
    try:
        bins = {"new": build(ROOT, ROOT / "target" / "bench")}
        if args.base:
            extract(args.base, work / "base")
            bins["base"] = build(work / "base", work / "target-base")
        for program in programs:
            (work / "trees" / program).mkdir(parents=True)
            print(f"building the {program} trees...", file=sys.stderr)
            run_cmd([str(bins["new"][program]), "build", str(work / "trees" / program)])
        print(f"{platform.platform()}, {os.cpu_count()} cores, {args.runs} runs, medians\n")
        for program in programs:
            rows = compare_program(program, bins, work / "trees" / program, args.runs, not args.base)
            print(f"### {program}\n")
            print(table(rows, args.base))
            print()
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print(f"took {time.time() - started:.0f} s", file=sys.stderr)


if __name__ == "__main__":
    main()
