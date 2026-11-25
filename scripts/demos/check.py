#!/usr/bin/env python3
"""Fails when a README demo says something the app no longer says."""
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import gen  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[2]
SOURCES = [
    "crates/attic/src/tui/ui.rs",
    "crates/attic/src/tui/app.rs",
    "crates/attic-core/src/notify.rs",
    "crates/attic-core/src/service.rs",
]


def source(root):
    return "".join((root / path).read_text() for path in SOURCES)


def missing(src, texts):
    """The entries of `texts` that `src` doesn't contain."""
    stale = []
    for key, text in texts.items():
        needle = f'send("{text}"' if key == "notification title" else text
        if needle not in src:
            stale.append(f"{key}: {text}")
    return stale


def main():
    stale = missing(source(ROOT), gen.UI_TEXT)
    for line in stale:
        print("stale demo text:", line)
    if stale:
        print("Update UI_TEXT in scripts/demos/gen.py, then run `make demos`.")
        sys.exit(1)
    print(f"demos match the app ({len(gen.UI_TEXT)} strings)")


if __name__ == "__main__":
    main()
