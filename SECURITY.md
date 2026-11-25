# Security

attic can move files to the Trash, so a bug that moves the wrong thing is a
security problem, not only an annoyance.

## Reporting

Please report a vulnerability privately, not in a public issue: on GitHub,
open the Security tab and choose "Report a vulnerability"
(https://github.com/sergiommarcial/attic/security/advisories/new).

Things that count:

- attic moving or deleting something the user did not pick, or doing so
  without the confirmation.
- Reading or writing outside attic's own database, config and log folders.
- Any network request. attic is meant to make none.
- The background service doing more than scanning, storing and notifying.
- The release workflow or the Homebrew formula shipping something other than
  the tagged source.

Include the attic version (`attic --version`), your macOS version, how you
installed it, and steps to reproduce.

## What to expect

I maintain attic alone, in spare time. I aim to reply within a week and to
tell you whether I can reproduce the problem. A fix gets a release and a
public advisory, and I will credit you if you want that.

## Supported versions

Only the latest release. attic is at 0.x, so fixes are not backported.
