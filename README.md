# KnotQ

KnotQ is a structured planning and calendar app. It treats each document line as a planning object that can remain as notes and procedures, or become calendar-backed events, assignments, and reminders.

## Development

```sh
cargo check --workspace
cargo test --workspace
./local/run-app.sh
```

Google Calendar development uses `local/.env`, which is intentionally ignored. Set `KNOTQ_GOOGLE_CLIENT_ID` and, when needed, `KNOTQ_GOOGLE_CLIENT_SECRET`.

## Release

Pushing a tag named `v*` runs the macOS release workflow and uploads arm64 and Intel app bundle archives.
