## What and why

<!-- What this changes, and the problem it solves. Link the issue. -->

## Performance

<!--
If this can make something slower or faster (scan, store, probe, suggestions,
the service loop, TUI redraws): numbers for a typical case, a stress case and
the nothing-found case, old against new, medians. Say how it was measured.
Otherwise write "no effect on speed".
-->

## Checklist

- [ ] `make ci` passes
- [ ] New behavior has tests, and a bug fix has a test that fails without it
- [ ] Benchmarked if it can affect speed; the numbers are above and in the README table or an ADR
- [ ] Every review finding is fixed, or filed as an issue and linked
- [ ] Docs updated (README, and an ADR for an architectural decision)
- [ ] Nothing new can delete a file outside the trashing module
