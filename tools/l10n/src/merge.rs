use crate::catalog::{parse_entries, write_catalog, Entry};
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Folds `l10n/partial/*.json` into `en.json`. Duplicate keys are fine when
/// the values agree (extraction agents share `common.*`); conflicting values
/// abort with a listing so a human resolves them. Partials are deleted after
/// a successful merge.
pub fn run(l10n_dir: &Path) -> Result<()> {
    let partial_dir = l10n_dir.join("partial");
    if !partial_dir.exists() {
        println!("no l10n/partial directory; nothing to merge");
        return Ok(());
    }

    let en_path = l10n_dir.join("en.json");
    let mut merged: BTreeMap<String, Entry> = parse_entries(&en_path)?;
    // key → source description, for conflict reporting.
    let mut sources: BTreeMap<String, String> = merged
        .keys()
        .map(|k| (k.clone(), "en.json".to_string()))
        .collect();

    let mut partial_paths: Vec<_> = fs::read_dir(&partial_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    partial_paths.sort();

    if partial_paths.is_empty() {
        println!("no partials to merge");
        return Ok(());
    }

    let mut conflicts = Vec::new();
    let mut added = 0usize;
    for path in &partial_paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for (key, entry) in parse_entries(path)? {
            match merged.get(&key) {
                None => {
                    merged.insert(key.clone(), entry);
                    sources.insert(key, name.clone());
                    added += 1;
                }
                Some(existing) if *existing == entry => {}
                Some(_) => conflicts.push(format!(
                    "  {key}: {name} disagrees with {}",
                    sources.get(&key).map(String::as_str).unwrap_or("en.json")
                )),
            }
        }
    }

    if !conflicts.is_empty() {
        bail!("conflicting values for {} key(s):\n{}", conflicts.len(), conflicts.join("\n"));
    }

    write_catalog(&en_path, &merged)?;
    for path in &partial_paths {
        fs::remove_file(path)?;
    }
    println!(
        "merged {added} new key(s) from {} partial(s); en.json now has {} keys",
        partial_paths.len(),
        merged.len()
    );
    Ok(())
}
