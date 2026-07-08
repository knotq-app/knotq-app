use crate::catalog::{load_all, placeholders, Entry, PLURAL_CATEGORIES};
use anyhow::{bail, Result};
use std::path::Path;

/// Checks every locale against `en.json`:
/// - keys must be well-formed (`lower.snake.case` dot segments)
/// - locales may LACK keys (falls back to English at runtime — warning only)
/// - locales must not have keys unknown to English (error: stale)
/// - plain entries must carry exactly English's placeholder set (error)
/// - plural entries: valid CLDR categories, `other` present, `{count}` the
///   only allowed placeholder, and plural-ness must match English (error)
pub fn run(l10n_dir: &Path) -> Result<()> {
    let catalogs = load_all(l10n_dir)?;
    let english = catalogs.english().clone();
    let mut errors = Vec::new();

    for key in english.keys() {
        let well_formed = !key.is_empty()
            && key.split('.').all(|segment| {
                !segment.is_empty()
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            });
        if !well_formed {
            errors.push(format!("en: malformed key {key:?}"));
        }
    }

    for (key, entry) in english.iter() {
        check_entry_shape("en", key, entry, &mut errors);
    }

    for locale in &catalogs.locales {
        if locale.code == "en" {
            continue;
        }
        let entries = &catalogs.by_locale[&locale.code];
        let mut missing = 0usize;
        for (key, en_entry) in &english {
            let Some(entry) = entries.get(key) else {
                missing += 1;
                continue;
            };
            check_entry_shape(&locale.code, key, entry, &mut errors);
            match (en_entry, entry) {
                (Entry::Plain(en_pattern), Entry::Plain(pattern)) => {
                    let mut expected = placeholders(en_pattern);
                    let mut got = placeholders(pattern);
                    expected.sort();
                    got.sort();
                    if expected != got {
                        errors.push(format!(
                            "{}: {key}: placeholders {got:?} != English {expected:?}",
                            locale.code
                        ));
                    }
                }
                (Entry::Plural(_), Entry::Plural(_)) => {}
                _ => errors.push(format!(
                    "{}: {key}: plural/plain shape differs from English",
                    locale.code
                )),
            }
        }
        for key in entries.keys() {
            if !english.contains_key(key) {
                errors.push(format!("{}: stale key {key} (not in en.json)", locale.code));
            }
        }
        let total = english.len();
        println!(
            "{:8} {:>4}/{} translated{}",
            locale.code,
            total - missing,
            total,
            if missing > 0 { "  (missing keys fall back to English)" } else { "" }
        );
    }

    if !errors.is_empty() {
        bail!("{} validation error(s):\n  {}", errors.len(), errors.join("\n  "));
    }
    println!("catalogs valid: {} keys, {} locales", english.len(), catalogs.locales.len());
    Ok(())
}

fn check_entry_shape(locale: &str, key: &str, entry: &Entry, errors: &mut Vec<String>) {
    let Entry::Plural(forms) = entry else {
        return;
    };
    if !forms.contains_key("other") {
        errors.push(format!("{locale}: {key}: plural entry missing required category `other`"));
    }
    for (category, pattern) in forms {
        if !PLURAL_CATEGORIES.contains(&category.as_str()) {
            errors.push(format!("{locale}: {key}: unknown plural category {category:?}"));
        }
        for name in placeholders(pattern) {
            if name != "count" {
                // Native plural resources (xcstrings/Android plurals) can only
                // thread the count through; keep other data out of plurals.
                errors.push(format!(
                    "{locale}: {key}: plural variants may only use {{count}}, found {{{name}}}"
                ));
            }
        }
    }
}
