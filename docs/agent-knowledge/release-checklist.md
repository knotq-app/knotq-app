# Release preparation checklist

Preparation and publication are separate. This checklist stops at a verified
artifact unless a human explicitly authorizes the store or hosting action.

## Before building

- Confirm the intended repository/branch and inspect both Git roots.
- Confirm version/build numbers, changelog, localization parity, and generated
  bindings are expected for the release.
- Run the sync property, mobile-core, and real-backend gates available in the
  environment. Missing backend checkout/credentials is a recorded blocker.

## Artifact checks

- iOS: archive, export, verify bundle ID/version/entitlements, retain dSYM UUID,
  and run the complete simulator test target.
- Android: run unit tests, lint, connected smoke tests, assemble the requested
  variants, and retain mapping/native symbols.
- Desktop: build each target, verify embedded MCP bridge, signatures,
  installer/layout tests, checksums, and update metadata.
- Website: validate localization keys, canonical/redirect paths, asset links,
  and preview output.

## Stop points

Do not upload, submit, publish, notarize with production credentials, change DNS,
or deploy backend code as an implied part of preparation. State exactly which
step remains and why.
