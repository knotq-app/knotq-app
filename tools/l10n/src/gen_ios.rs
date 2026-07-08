use crate::catalog::{Catalogs, Entry, TargetConfig};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

/// Emits a single `Localizable.xcstrings` string catalog holding every locale.
///
/// Plain entries keep their literal `{name}` placeholders — `L10n.swift`
/// substitutes them at runtime, so no printf-specifier conversion can go
/// wrong in translation. Plural entries become xcstrings plural variations
/// with `{count}` rewritten to `%lld`, which is what drives iOS's
/// category selection in `String.localizedStringWithFormat`.
pub fn run(catalogs: &Catalogs, config: &TargetConfig) -> Result<()> {
    let mut strings = Map::new();

    for key in catalogs.english().keys() {
        let mut localizations = Map::new();
        for locale in &catalogs.locales {
            let Some(entry) = catalogs.by_locale[&locale.code].get(key) else {
                continue;
            };
            let localization = match entry {
                Entry::Plain(pattern) => json!({
                    "stringUnit": { "state": "translated", "value": pattern }
                }),
                Entry::Plural(forms) => {
                    let variations: Map<String, Value> = forms
                        .iter()
                        .map(|(category, pattern)| {
                            (
                                category.clone(),
                                json!({
                                    "stringUnit": {
                                        "state": "translated",
                                        "value": pattern.replace("{count}", "%lld"),
                                    }
                                }),
                            )
                        })
                        .collect();
                    json!({ "variations": { "plural": variations } })
                }
            };
            // xcstrings uses the raw code; Apple accepts zh-Hans/zh-Hant/pt-BR as-is.
            localizations.insert(locale.code.clone(), localization);
        }
        strings.insert(
            key.clone(),
            json!({ "extractionState": "manual", "localizations": localizations }),
        );
    }

    let catalog = json!({
        "sourceLanguage": "en",
        "version": "1.0",
        "strings": strings,
    });
    let mut out = serde_json::to_string_pretty(&catalog)?;
    out.push('\n');
    if let Some(parent) = config.ios_xcstrings.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config.ios_xcstrings, out)
        .with_context(|| format!("writing {}", config.ios_xcstrings.display()))?;
    println!("wrote {}", config.ios_xcstrings.display());
    Ok(())
}
