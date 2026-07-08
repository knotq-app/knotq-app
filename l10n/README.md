# KnotQ Localization (l10n)

Single source of truth for every user-facing string across desktop, mobile
(iOS/Android), the shared Rust cores, and the website.

## Layout

- `en.json` — canonical catalog (English). Every user-facing string lives here.
- `<locale>.json` — one translation catalog per locale (same keys as `en.json`).
- `locales.json` — registry of supported locales (`code`, `name`, `native`, `rtl`).
- `partial/` — scratch area used while extracting strings; merged into `en.json`
  by `l10n-gen merge` and then deleted. Not shipped.

## Catalog format

Flat JSON object. Keys are dot-namespaced, lowercase snake_case segments:

```json
{
  "sidebar.context.new_item": "New Item",
  "sync.popover.last_synced": "Last synced {when}.",
  "archive.delete_count": {
    "one": "This permanently deletes {count} archived item.",
    "other": "This permanently deletes {count} archived items."
  }
}
```

- **Namespaces** (first segment): `common`, `menu`, `sidebar`, `settings`,
  `calendar`, `editor`, `sync`, `account`, `archive`, `daily`, `search`,
  `notifications`, `update`, `onboarding`, `google`, `event`, `mobile`,
  `web` (website-only), plus new ones as needed. Shared strings that appear on
  several surfaces go under `common`.
- **Placeholders** are `{name}` (word characters only). Every locale must keep
  exactly the same placeholder set per key — `l10n-gen validate` enforces this.
  `{{` and `}}` escape literal braces.
- **Plurals**: an object value with CLDR cardinal categories (`zero`, `one`,
  `two`, `few`, `many`, `other`); `other` is required. Plural entries must use
  the `{count}` placeholder. English only needs `one`/`other`; translators fill
  the categories their language uses.

## Runtimes

- **Rust (desktop app + mobile core)**: crate `knotq-l10n`
  (`app/shared/l10n`). Catalogs are embedded at compile time.
  `t(key) -> &'static str`, `t_with(key, &[("name", value)])`,
  `t_count(key, n)`, `set_locale(code)`. Missing key → English → key itself.
- **iOS**: generated `Localizable.xcstrings` + `L10n.swift`
  (`L10n.t(key)`, `L10n.t(key, args)`, `L10n.plural(key, count)`).
- **Android**: generated `res/values-<qual>/strings.xml` (+ `<plurals>`) and
  `L10n.kt` (key → `R.string` map with the same helpers).
- **Website**: generated `website/js/i18n/<locale>.js` + hand-written runtime
  `website/js/i18n.js` (translates `data-i18n` attributes, exposes
  `t(key, params)`, plurals via `Intl.PluralRules`).

## Workflow

1. Add/edit English strings in `en.json` (or drop a `partial/*.json` during a
   bulk extraction and run `cargo run -p l10n-gen -- merge`).
2. Translate: update each `<locale>.json` (missing keys fall back to English at
   runtime, so partial translations are safe to ship).
3. `cargo run -p l10n-gen -- validate` — checks key parity, placeholder parity,
   plural-category sanity across all locales.
4. `cargo run -p l10n-gen -- generate` — regenerates the iOS/Android/website
   artifacts. Rust picks up catalog changes on the next `cargo build`
   (the crate's `build.rs` watches this directory).

Adding a locale = add it to `locales.json`, create `<locale>.json`, run
`generate`. Nothing else to touch.
