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

Pushing a tag named `v*` builds and publishes:

- a universal macOS DMG (`arm64` + `x86_64`)
- a Windows x64 MSIX plus a portable zip
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

- `WINDOWS_PUBLISHER`: certificate subject, for example `CN=Enigmadux`
- `WINDOWS_PUBLISHER_DISPLAY_NAME`: optional display name
