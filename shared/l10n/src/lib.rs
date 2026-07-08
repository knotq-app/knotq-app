//! Runtime string catalog shared by the desktop app and the mobile core.
//!
//! Catalogs from `app/l10n/*.json` are embedded at compile time. Lookup is by
//! flat dot-namespaced key with graceful fallback: current locale → English →
//! the key itself (so a missing translation can never panic or blank the UI).
//!
//! Locale data is parsed once per locale and leaked; entries therefore hand out
//! `&'static str`, which flows into GPUI's `SharedString` without copying.

mod plural;

pub use plural::plural_category;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_catalogs.rs"));
}

/// A locale offered in language pickers.
#[derive(Clone, Debug)]
pub struct LocaleInfo {
    pub code: &'static str,
    /// English name ("German").
    pub name: &'static str,
    /// Autonym ("Deutsch") — what pickers should display.
    pub native: &'static str,
    pub rtl: bool,
}

enum Entry {
    Plain(String),
    /// CLDR cardinal category → pattern. `other` is guaranteed by validation.
    Plural(HashMap<String, String>),
}

struct Catalog {
    code: &'static str,
    entries: HashMap<String, Entry>,
}

fn parse_catalog(code: &'static str, raw: &str) -> Catalog {
    let value: serde_json::Value =
        serde_json::from_str(raw).unwrap_or_else(|err| panic!("l10n catalog {code}: {err}"));
    let mut entries = HashMap::new();
    if let serde_json::Value::Object(map) = value {
        for (key, value) in map {
            let entry = match value {
                serde_json::Value::String(s) => Entry::Plain(s),
                serde_json::Value::Object(forms) => Entry::Plural(
                    forms
                        .into_iter()
                        .filter_map(|(cat, v)| Some((cat, v.as_str()?.to_string())))
                        .collect(),
                ),
                _ => continue,
            };
            entries.insert(key, entry);
        }
    }
    Catalog { code, entries }
}

fn catalogs() -> &'static Mutex<HashMap<&'static str, &'static Catalog>> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, &'static Catalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn catalog_for(code: &'static str) -> &'static Catalog {
    let mut cache = catalogs().lock().unwrap();
    cache.entry(code).or_insert_with(|| {
        let raw = embedded::CATALOGS
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, raw)| *raw)
            .unwrap_or("{}");
        Box::leak(Box::new(parse_catalog(code, raw)))
    })
}

fn english() -> &'static Catalog {
    catalog_for("en")
}

fn current() -> &'static RwLock<&'static Catalog> {
    static CURRENT: OnceLock<RwLock<&'static Catalog>> = OnceLock::new();
    CURRENT.get_or_init(|| RwLock::new(english()))
}

/// All locales from `locales.json`, in registry order (English first).
pub fn available_locales() -> &'static [LocaleInfo] {
    static LOCALES: OnceLock<Vec<LocaleInfo>> = OnceLock::new();
    LOCALES.get_or_init(|| {
        let value: serde_json::Value = serde_json::from_str(embedded::LOCALES_JSON)
            .expect("l10n locales.json is invalid JSON");
        let leak = |v: &serde_json::Value| -> &'static str {
            Box::leak(v.as_str().unwrap_or_default().to_string().into_boxed_str())
        };
        value
            .as_array()
            .expect("locales.json must be an array")
            .iter()
            .map(|entry| LocaleInfo {
                code: leak(&entry["code"]),
                name: leak(&entry["name"]),
                native: leak(&entry["native"]),
                rtl: entry["rtl"].as_bool().unwrap_or(false),
            })
            .collect()
    })
}

/// Maps an arbitrary BCP-47-ish tag (`de-AT`, `zh_CN`, `pt`) onto a supported
/// locale code, or `None` if nothing plausible matches.
pub fn resolve_locale(tag: &str) -> Option<&'static str> {
    let tag = tag.trim().replace('_', "-");
    if tag.is_empty() {
        return None;
    }
    let lower = tag.to_ascii_lowercase();
    let locales = available_locales();

    if let Some(info) = locales.iter().find(|l| l.code.eq_ignore_ascii_case(&tag)) {
        return Some(info.code);
    }

    // Chinese needs script-aware mapping before falling back to bare language.
    if lower == "zh" || lower.starts_with("zh-") {
        let traditional = ["hant", "tw", "hk", "mo"]
            .iter()
            .any(|part| lower.split('-').any(|seg| seg == *part));
        return Some(if traditional { "zh-Hant" } else { "zh-Hans" });
    }

    let language = lower.split('-').next().unwrap_or(&lower);
    match language {
        // Bare "pt" is ambiguous; pick the larger population.
        "pt" => return Some("pt-BR"),
        // Norwegian macrolanguage and legacy codes.
        "no" | "nn" => return Some("nb"),
        "iw" => return Some("he"),
        "in" => return Some("id"),
        _ => {}
    }

    locales
        .iter()
        .find(|l| {
            l.code
                .split('-')
                .next()
                .is_some_and(|base| base.eq_ignore_ascii_case(language))
        })
        .map(|l| l.code)
}

/// Switches the active locale. Accepts any tag `resolve_locale` understands;
/// unknown tags leave English active. Returns the locale actually selected.
pub fn set_locale(tag: &str) -> &'static str {
    let code = resolve_locale(tag).unwrap_or("en");
    *current().write().unwrap() = catalog_for(code);
    code
}

/// The active locale code (`"en"` until `set_locale` succeeds).
pub fn locale() -> &'static str {
    current().read().unwrap().code
}

/// Whether the active locale is right-to-left.
pub fn locale_is_rtl() -> bool {
    let code = locale();
    available_locales()
        .iter()
        .find(|l| l.code == code)
        .is_some_and(|l| l.rtl)
}

/// The OS UI language as a raw tag, if detectable.
#[cfg(feature = "system-locale")]
pub fn system_locale_tag() -> Option<String> {
    sys_locale::get_locale()
}

fn raw_lookup(key: &str) -> Option<&'static Entry> {
    let active = *current().read().unwrap();
    if let Some(entry) = active.entries.get(key) {
        return Some(entry);
    }
    english().entries.get(key)
}

/// Translates `key`. Falls back to English, then to the key itself.
pub fn t(key: &str) -> &'static str {
    match raw_lookup(key) {
        Some(Entry::Plain(s)) => s,
        Some(Entry::Plural(forms)) => forms
            .get("other")
            .map(String::as_str)
            .unwrap_or_else(|| leak_key(key)),
        None => leak_key(key),
    }
}

/// A key with no catalog entry anywhere. Leaking keeps the return type uniform;
/// this only happens on programmer error and the debug assert makes it loud.
fn leak_key(key: &str) -> &'static str {
    debug_assert!(false, "l10n: missing key {key:?} (not in en.json)");
    Box::leak(key.to_string().into_boxed_str())
}

/// Translates `key` and substitutes `{name}` placeholders. `{{`/`}}` escape
/// literal braces. Unknown placeholders are left verbatim so a stale
/// translation degrades visibly rather than panicking.
pub fn t_with(key: &str, args: &[(&str, &str)]) -> String {
    substitute(t(key), args)
}

/// Translates a plural `key` for `count`, substituting `{count}`.
pub fn t_count(key: &str, count: i64) -> String {
    let pattern = match raw_lookup(key) {
        Some(Entry::Plural(forms)) => {
            let category = plural_category(locale(), count);
            forms
                .get(category)
                .or_else(|| forms.get("other"))
                .map(String::as_str)
                .unwrap_or_else(|| leak_key(key))
        }
        Some(Entry::Plain(s)) => s,
        None => leak_key(key),
    };
    substitute(pattern, &[("count", &count.to_string())])
}

fn substitute(pattern: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let name: String = chars.clone().take_while(|c| *c != '}').collect();
                if let Some((_, value)) = args.iter().find(|(n, _)| *n == name) {
                    for _ in 0..name.len() + 1 {
                        chars.next();
                    }
                    out.push_str(value);
                } else {
                    out.push('{');
                }
            }
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution_and_escapes() {
        assert_eq!(
            substitute("Last synced {when}.", &[("when", "3m ago")]),
            "Last synced 3m ago."
        );
        assert_eq!(substitute("{{literal}} {x}", &[("x", "v")]), "{literal} v");
        assert_eq!(substitute("keep {unknown}", &[]), "keep {unknown}");
    }

    #[test]
    fn missing_key_falls_back_to_key_in_release() {
        // Debug builds assert; this documents the release-mode contract.
        if !cfg!(debug_assertions) {
            assert_eq!(t("no.such.key"), "no.such.key");
        }
    }

    #[test]
    fn locale_resolution() {
        assert_eq!(resolve_locale("en-US"), Some("en"));
        assert_eq!(resolve_locale("de_AT"), Some("de"));
        assert_eq!(resolve_locale("pt"), Some("pt-BR"));
        assert_eq!(resolve_locale("pt-PT"), Some("pt-PT"));
        assert_eq!(resolve_locale("zh-TW"), Some("zh-Hant"));
        assert_eq!(resolve_locale("zh-CN"), Some("zh-Hans"));
        assert_eq!(resolve_locale("zh"), Some("zh-Hans"));
        assert_eq!(resolve_locale("no"), Some("nb"));
        assert_eq!(resolve_locale("iw-IL"), Some("he"));
        assert_eq!(resolve_locale("klingon"), None);
        assert_eq!(resolve_locale(""), None);
    }

    #[test]
    fn registry_parses_and_starts_english() {
        assert!(available_locales().iter().any(|l| l.code == "en"));
        assert_eq!(t("settings.language.label"), "Language");
    }
}
