# Release fixtures

Each `vX.Y.Z/` directory is a **complete data directory captured by running that
release's own code**, not this build's idea of what that release used to write.
`tests/upgrade_from_release.rs` opens every one of them with the current build
and asserts that everything comes back.

That distinction is the point. A hand-written fixture only ever encodes what we
*believe* an old build wrote, so it agrees with our mistakes; these bytes were
produced by the tagged binary and cannot.

## What a fixture contains

A workspace with the awkward cases rather than the easy ones: nested folders, an
archived scheme, every marker, indents, dates, an RRULE, a done occurrence with a
notification offset, an image line, a table line, unicode and markdown
metacharacters, two daily-queue days — plus the state a user never sees and
cannot rebuild: `sync-crdt-state.json` (the CRDT document identity), a
`sync-state.json` holding cursors and one *unpushed* edit, and a `settings.json`
with a signed-in sync account and a linked Google account.

Excluded: `.knotq-history/` and `workspace/backups/`, both of which the
workspace's own `.gitignore` keeps out of git anyway, and neither of which the
upgrade path reads.

## Capturing a fixture from a release

Do this **before** shipping a format change, from the tag the change is going out
to — the fixture must be written by the last release users are actually on.

```sh
git worktree add /tmp/knotq-vX.Y.Z vX.Y.Z
cp desktop/storage-json/tests/capture_fixture.rs \
   /tmp/knotq-vX.Y.Z/desktop/storage-json/tests/
cd /tmp/knotq-vX.Y.Z
KNOTQ_FIXTURE_OUT=/tmp/fixture cargo test -p knotq-storage-json \
    --test capture_fixture -- --nocapture
```

Then copy `/tmp/fixture` to `desktop/storage-json/tests/fixtures/vX.Y.Z/`, minus
`.knotq-history/` and `workspace/backups/`, and commit it. If the capture does
not compile against the older tree, adjust it there for that release's API —
what matters is the bytes it produces, not that one file compiles everywhere.

The tests discover fixtures by directory name, so nothing else needs changing.
Old fixtures are kept: users skip versions, and an install that has not been
opened in a year still has to survive the upgrade.
