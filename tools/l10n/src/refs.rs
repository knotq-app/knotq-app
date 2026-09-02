//! Cross-checks the catalog against the keys the source code actually asks for.
//!
//! `validate` proves the locales agree with `en.json`. It cannot see the other
//! direction: a key that code still calls but the catalog never had. Both
//! runtimes resolve a miss to the key itself — `NSLocalizedString` returns its
//! argument, and Android's generated map ends in `?: key` — so the app renders
//! a literal `settings.timing.title` where a label belongs, with nothing failing
//! anywhere. That is exactly how regenerating `Localizable.xcstrings` from the
//! catalog silently dropped nine hand-maintained keys and shipped raw key names
//! into the iOS Settings screen.
//!
//! So this runs before every `generate`: the artifacts are not rewritten while
//! source code references a key the catalog cannot serve.
//!
//! Trees that are not checked out are skipped, not failed — the mobile sources
//! live in a sibling repository that CI for this one does not clone.

use crate::catalog::Entry;
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One language's way of naming a catalog key, and where to look for it.
struct Surface {
    label: &'static str,
    root: PathBuf,
    extension: &'static str,
    /// Text that must appear immediately before the quoted key.
    call_prefixes: &'static [&'static str],
    /// Generated files that name every key by construction, so scanning them
    /// would just echo the catalog back at itself.
    skip_file: Option<&'static str>,
    /// Subdirectory of `root` to leave alone.
    skip_dir: Option<&'static str>,
    /// The call takes a context/receiver before the key.
    context_first: bool,
}

/// Keys are `lower.snake.case` with at least one dot (see `l10n/README.md`).
/// Anything else quoted at a call site is some other string argument, and a
/// scanner that guessed otherwise would fail the build over ordinary code.
fn looks_like_a_key(text: &str) -> bool {
    text.contains('.')
        && text.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// Every catalog key passed to one of `surface`'s call forms.
///
/// A call is matched on its opening `name(`, at an identifier boundary, and the
/// key is the first string literal after it — with an optional leading argument
/// skipped for the surfaces that thread a context through (Android spells the
/// same call `L10n.t(this, "key")`, `L10n.t(activity, "key")`, and so on, and a
/// scanner that enumerated those spellings would quietly miss the next one).
fn keys_in(text: &str, surface: &Surface) -> Vec<String> {
    let bytes = text.as_bytes();
    let is_ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut found = Vec::new();
    for prefix in surface.call_prefixes {
        let mut from = 0usize;
        while let Some(hit) = text[from..].find(prefix) {
            let start = from + hit;
            let mut at = start + prefix.len();
            from = at;
            // `t(` must not be the tail of `insert(`; `looks_like_a_key` would
            // usually save us, but a word boundary is what we actually mean.
            if start > 0 && is_ident(bytes[start - 1]) {
                continue;
            }
            let skip_spaces = |at: &mut usize| {
                while bytes.get(*at).is_some_and(|c| c.is_ascii_whitespace()) {
                    *at += 1;
                }
            };
            skip_spaces(&mut at);
            if surface.context_first && bytes.get(at).is_some_and(|c| is_ident(*c)) {
                while bytes.get(at).is_some_and(|c| is_ident(*c) || *c == b'.') {
                    at += 1;
                }
                skip_spaces(&mut at);
                if bytes.get(at) != Some(&b',') {
                    continue;
                }
                at += 1;
                skip_spaces(&mut at);
            }
            if bytes.get(at) != Some(&b'"') {
                continue;
            }
            at += 1;
            let Some(end) = text[at..].find('"') else {
                continue;
            };
            let key = &text[at..at + end];
            // A key never spans lines or holds an escape.
            if key.contains('\n') || key.contains('\\') {
                continue;
            }
            if looks_like_a_key(key) {
                found.push(key.to_string());
            }
        }
    }
    found
}

fn walk(dir: &Path, surface: &Surface, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // Build output restates the catalog and dwarfs the source tree.
            if name == "target" || name == "build" || name == ".git" {
                continue;
            }
            if surface.skip_dir.is_some_and(|skip| name == skip) {
                continue;
            }
            walk(&path, surface, out);
        } else if path.extension().is_some_and(|e| e == surface.extension)
            && surface.skip_file.is_none_or(|skip| name != skip)
        {
            out.push(path);
        }
    }
}

/// Fails if any surface references a key `en.json` does not define.
pub fn run(english: &BTreeMap<String, Entry>, root: &Path, l10n_dir: &Path) -> Result<()> {
    // Derived from config.json's own paths so the two cannot drift: the iOS
    // sources are the directory holding the catalog's group, and the Android
    // sources sit beside the generated `L10n.kt`.
    let config = crate::catalog::TargetConfig::load(l10n_dir, root)?;
    let ios_root = config
        .ios_xcstrings
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let kotlin_root = config.android_l10n_kt.parent().map(Path::to_path_buf);

    let mut surfaces = vec![
        Surface {
            label: "desktop (Rust)",
            root: root.join("desktop"),
            extension: "rs",
            // `t` is aliased to `tr` at most call sites; both take the key first.
            call_prefixes: &["t(", "tr(", "t_with(", "tr_with(", "t_count(", "tr_count("],
            skip_file: None,
            skip_dir: None,
            context_first: false,
        },
        Surface {
            label: "shared cores (Rust)",
            root: root.join("shared"),
            extension: "rs",
            call_prefixes: &["t(", "tr(", "t_with(", "tr_with(", "t_count(", "tr_count("],
            skip_file: None,
            // The `knotq-l10n` crate defines `t`; its own unit tests call it
            // with deliberately absent keys to prove the fallback behaviour.
            skip_dir: Some("l10n"),
            context_first: false,
        },
    ];
    if let Some(ios_root) = ios_root {
        surfaces.push(Surface {
            label: "iOS (Swift)",
            root: ios_root,
            extension: "swift",
            call_prefixes: &["L10n.t(", "L10n.plural("],
            skip_file: None,
            skip_dir: None,
            context_first: false,
        });
    }
    if let Some(kotlin_root) = kotlin_root {
        surfaces.push(Surface {
            label: "Android (Kotlin)",
            root: kotlin_root,
            extension: "kt",
            call_prefixes: &["L10n.t(", "L10n.plural("],
            skip_file: Some("L10n.kt"),
            skip_dir: None,
            // Android threads a Context first: `L10n.t(this, "key")`.
            context_first: true,
        });
    }

    let mut missing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for surface in &surfaces {
        if !surface.root.is_dir() {
            println!("{:22} not checked out, skipped", surface.label);
            continue;
        }
        let mut files = Vec::new();
        walk(&surface.root, surface, &mut files);
        let mut referenced = BTreeSet::new();
        for file in &files {
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            for key in keys_in(&text, surface) {
                referenced.insert(key.clone());
                if !english.contains_key(&key) {
                    let where_ = file
                        .strip_prefix(root)
                        .unwrap_or(file)
                        .display()
                        .to_string();
                    missing.entry(key).or_default().insert(where_);
                }
            }
        }
        println!(
            "{:22} {} keys referenced across {} files",
            surface.label,
            referenced.len(),
            files.len()
        );
    }

    if !missing.is_empty() {
        let mut report = String::new();
        for (key, files) in &missing {
            report.push_str(&format!(
                "\n  {key} — referenced by {}",
                files.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        bail!(
            "{} key(s) referenced by source code but absent from en.json. Both mobile \
             runtimes render a missing key as the key itself, so this would ship literal \
             key names into the UI. Add them to l10n/en.json (and every locale) rather \
             than to a generated artifact:{report}",
            missing.len()
        );
    }
    Ok(())
}
