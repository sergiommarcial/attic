# Contributing

attic is a small project and fixes are welcome. For anything bigger than a
small fix, open an issue first so we can agree on the shape before you spend
the time.

attic is for macOS only and builds with Rust 1.96 or newer.

## Ground rules

- Anything that removes a file goes through one place, the trashing module,
  and only from the terminal app after a confirmation. A change that touches
  that needs a discussion first.
- The background service never deletes anything.
- Nothing leaves the machine: no network calls, no telemetry.

## Building and testing

```sh
make install-tools   # cargo-deny, cargo-machete, actionlint, shellcheck
make ci              # formatting, clippy, tests, linters, deny, machete
```

`make help` lists the other targets. While you work, `make test-fast
TESTS="store scan"` runs the library's unit tests plus the named
integration files, which is much quicker than the whole suite.

Tests run on temporary folders, never on your real home folder, and never
move anything to your real Trash. New behavior comes with tests, and a bug
fix comes with a test that fails without the fix. The terminal app is tested
through its key handler and ratatui's `TestBackend`.

## Decisions

Design decisions live in `docs/adr/`. If your change makes a new
architectural decision (a schema change, a new way the service learns about
changes, a new action on user files), add an ADR for it in the same pull
request, with the two diagrams the others have. If it contradicts an existing
ADR, say so in the pull request.

## Pull requests

- Keep it to one thing, and make sure `make ci` passes.
- Commit messages are one short line.
- Write docs and comments the way you would explain it to a colleague. Plain
  sentences, no marketing.

By contributing you agree that your work is released under the MIT license
in `LICENSE`.
