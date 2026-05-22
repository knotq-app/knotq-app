# KnotQ

KnotQ is a local-first desktop productivity app where your notes and calendar live in the same workspace. Write plans as structured documents, schedule any line, and see the result immediately on a week calendar without sending your data to a cloud account.

<p align="center">
  <img src="https://www.knotq.com/img/knotq-calendar.png" alt="KnotQ weekly calendar showing events, assignments, and reminders" width="880">
</p>

## Why KnotQ

Most planning tools split your thinking across a notes app, a task manager, and a calendar. KnotQ keeps those pieces together. A line can stay as a note, become a checkbox, get a due date, turn into a reminder, or become a scheduled event block.

- **Notes that schedule themselves:** add start and end times to any line to make events, assignments, and reminders.
- **A calendar with source context:** calendar items link back to the exact line they came from.
- **Daily Queue:** keep a focused day list for quick tasks, notes, and carry-over work.
- **Nested task details:** indented child lines become event descriptions.
- **Local-first storage:** schemes, tasks, settings, and daily notes stay on your device.
- **Optional Google Calendar import:** view Google Calendar events read-only alongside your KnotQ workspace.

## Product Tour

### Write in schemes

Schemes are markdown-like documents for projects, classes, workstreams, and personal plans. Use headings, checkboxes, bullets, numbered lists, indentation, formatting, and inline dates without leaving the editor.

<p align="center">
  <img src="https://www.knotq.com/img/knotq-editor.png" alt="KnotQ scheme editor showing structured notes, tasks, and inline dates" width="880">
</p>

### Plan each day

The Daily Queue gives every day its own page. Incomplete items can carry forward, while completed work stays behind.

<p align="center">
  <img src="https://www.knotq.com/img/knotq-daily.png" alt="KnotQ Daily Queue showing tasks and notes for the day" width="880">
</p>

### Schedule from any line

Use the inline date picker to set a start time, end time, or both. KnotQ turns the line into the right calendar object.

<p align="center">
  <img src="https://www.knotq.com/img/knotq-date-picker.png" alt="KnotQ date picker used from inside the editor" width="880">
</p>

## Download

Download KnotQ from the latest GitHub release:

https://github.com/knotq-app/knotq-app/releases

Linux users can install from the release tarball with:

```sh
curl -fsSL https://knotq.com/install.sh | sh
```

## Community

Join the KnotQ Discord for questions, feedback, and release discussion:

https://discord.gg/zyeHB77scg

## Development

```sh
cargo check --workspace
cargo test --workspace
./local/run-app.sh
```

Google Calendar development uses `local/.env`, which is intentionally ignored. Set `KNOTQ_GOOGLE_CLIENT_ID` and, when needed, `KNOTQ_GOOGLE_CLIENT_SECRET`.

## Release

Pushing a tag named `v*` builds and publishes:

- a universal macOS DMG (`arm64` + `x86_64`)
- a Windows x64 MSIX and Inno Setup installer
- a Linux x86_64 tarball plus `install.sh`

macOS notarization is enabled when these secrets are present:

- `MACOS_CERTIFICATE_P12`: base64-encoded Developer ID Application `.p12`
- `MACOS_CERTIFICATE_PASSWORD`
- `CODESIGN_IDENTITY`: the Developer ID Application identity name
- `APPLE_NOTARY_APPLE_ID`, `APPLE_NOTARY_PASSWORD`, `APPLE_NOTARY_TEAM_ID`
- `MACOS_PROVISIONING_PROFILE`: optional base64-encoded `.provisionprofile`, required if the app uses a capability that must be provisioned, such as Time Sensitive Notifications

Local macOS bundle signing can use the same entitlement/profile path with `local/.env`:

```sh
KNOTQ_CODESIGN_IDENTITY="Developer ID Application: Example, Inc. (TEAMID)"
KNOTQ_PROVISIONING_PROFILE="/path/to/KnotQ.provisionprofile"
KNOTQ_ENTITLEMENTS_PATH="bundling/macos/KnotQ.entitlements"
```

Windows package identity can be configured with:

- `WINDOWS_PUBLISHER`: Microsoft Store publisher subject from Partner Center
- `WINDOWS_PUBLISHER_DISPLAY_NAME`: optional display name
