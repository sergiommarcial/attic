# Architecture decisions

| ADR | Decision | Status |
|---|---|---|
| [ADR-001](ADR-001-store-and-events.md) | One SQLite store kept current from FSEvents, disk read before each short write | Accepted |
| [ADR-002](ADR-002-fsevents-ffi.md) | FSEvents through a few lines of our own FFI | Accepted |
| [ADR-003](ADR-003-suggestion-selection.md) | How suggestions are picked and counted | Accepted |
| [ADR-004](ADR-004-releases-and-homebrew.md) | Tag-driven releases; CI writes the Homebrew formula into a public tap | Accepted |
| [ADR-005](ADR-005-coalescing-rescans.md) | Rescans of a busy folder are coalesced: once it is quiet, or once a minute | Accepted |
