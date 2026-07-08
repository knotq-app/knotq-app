use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// One catalog entry: a plain pattern or CLDR plural variants.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Plain(String),
    /// category → pattern; validation guarantees `other` exists.
    Plural(BTreeMap<String, String>),
}

pub struct LocaleInfo {
    pub code: String,
    /// Autonym ("Deutsch") shown in language pickers.
    pub native: String,
    pub rtl: bool,
}

pub struct Catalogs {
    /// Registry order from `locales.json` (English first).
    pub locales: Vec<LocaleInfo>,
    /// locale code → key → entry. Every registry locale has a map (possibly
    /// empty for not-yet-translated locales).
    pub by_locale: BTreeMap<String, BTreeMap<String, Entry>>,
}

impl Catalogs {
    pub fn english(&self) -> &BTreeMap<String, Entry> {
        &self.by_locale["en"]
    }
}

pub const PLURAL_CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

pub fn parse_entries(path: &Path) -> Result<BTreeMap<String, Entry>> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: Value =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    let Value::Object(map) = value else {
        bail!("{} must be a JSON object", path.display());
    };
    let mut entries = BTreeMap::new();
    for (key, value) in map {
        let entry = match value {
            Value::String(s) => Entry::Plain(s),
            Value::Object(forms) => {
                let mut variants = BTreeMap::new();
                for (category, v) in forms {
                    let Value::String(s) = v else {
                        bail!("{}: {key}: plural variant {category} must be a string", path.display());
                    };
                    variants.insert(category, s);
                }
                Entry::Plural(variants)
            }
            _ => bail!("{}: {key}: value must be a string or plural object", path.display()),
        };
        entries.insert(key, entry);
    }
    Ok(entries)
}

pub fn load_all(l10n_dir: &Path) -> Result<Catalogs> {
    let registry: Value = serde_json::from_str(
        &fs::read_to_string(l10n_dir.join("locales.json")).context("reading locales.json")?,
    )
    .context("parsing locales.json")?;
    let mut locales = Vec::new();
    for entry in registry.as_array().context("locales.json must be an array")? {
        locales.push(LocaleInfo {
            code: entry["code"]
                .as_str()
                .context("locales.json entry missing code")?
                .to_string(),
            native: entry["native"].as_str().unwrap_or_default().to_string(),
            rtl: entry["rtl"].as_bool().unwrap_or(false),
        });
    }

    let mut by_locale = BTreeMap::new();
    for locale in &locales {
        let path = l10n_dir.join(format!("{}.json", locale.code));
        let entries = if path.exists() {
            parse_entries(&path)?
        } else {
            BTreeMap::new()
        };
        by_locale.insert(locale.code.clone(), entries);
    }
    if !by_locale.contains_key("en") {
        bail!("locales.json must include en");
    }
    Ok(Catalogs { locales, by_locale })
}

/// The `{name}` placeholders in a pattern, in order of first appearance.
/// `{{`/`}}` escapes are skipped.
pub fn placeholders(pattern: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            '{' => {
                let mut name = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    name.push(c);
                }
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !out.contains(&name)
                {
                    out.push(name);
                }
            }
            _ => {}
        }
    }
    out
}

/// Serializes entries as the canonical sorted, two-space-indented catalog file.
pub fn write_catalog(path: &Path, entries: &BTreeMap<String, Entry>) -> Result<()> {
    let mut map = serde_json::Map::new();
    for (key, entry) in entries {
        let value = match entry {
            Entry::Plain(s) => Value::String(s.clone()),
            Entry::Plural(forms) => Value::Object(
                forms
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            ),
        };
        map.insert(key.clone(), value);
    }
    let mut out = serde_json::to_string_pretty(&Value::Object(map))?;
    out.push('\n');
    fs::write(path, out).with_context(|| format!("writing {}", path.display()))
}

/// Output locations for generated artifacts, from `l10n/config.json`.
/// Paths are relative to the app repo root.
pub struct TargetConfig {
    pub ios_xcstrings: PathBuf,
    pub android_res_dir: PathBuf,
    pub android_l10n_kt: PathBuf,
    pub website_i18n_dir: PathBuf,
}

impl TargetConfig {
    pub fn load(l10n_dir: &Path, root: &Path) -> Result<Self> {
        let raw = fs::read_to_string(l10n_dir.join("config.json"))
            .context("reading l10n/config.json")?;
        let value: Value = serde_json::from_str(&raw).context("parsing l10n/config.json")?;
        let path = |key: &str| -> Result<PathBuf> {
            Ok(root.join(value[key].as_str().with_context(|| format!("config.json missing {key}"))?))
        };
        Ok(Self {
            ios_xcstrings: path("ios_xcstrings")?,
            android_res_dir: path("android_res_dir")?,
            android_l10n_kt: path("android_l10n_kt")?,
            website_i18n_dir: path("website_i18n_dir")?,
        })
    }
}
