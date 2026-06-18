//! One-off migration: convert a KnotQ data directory from the legacy
//! human-readable markdown `.knotq` format to the new XML format.
//!
//! Usage:
//!   cargo run -p knotq-storage-json --example migrate_to_xml -- <workspace_dir> [--dry-run]
//!
//! Converts `schemes/*.knotq`, `daily_queue/**/*.knotq`, and any
//! `backups/**/schemes/*.knotq` in place. Files that already start with
//! `<?xml` are left untouched, so the tool is safe to re-run.

use std::path::{Path, PathBuf};

use knotq_model::{Scheme, SchemeId};
use knotq_storage_json::{
    decode_legacy_markdown_items, decode_xml_items, encode_scheme_to_xml, repair_scheme_file_format,
};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: migrate_to_xml <workspace_dir> [--dry-run]");
        std::process::exit(2);
    };
    let dry_run = args.any(|a| a == "--dry-run");
    let dir = PathBuf::from(dir);

    let mut targets = Vec::new();
    collect_knotq_files(&dir.join("schemes"), &mut targets);
    collect_knotq_files(&dir.join("daily_queue"), &mut targets);
    collect_knotq_files(&dir.join("backups"), &mut targets);

    let mut converted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for path in &targets {
        match convert_file(path, dry_run) {
            Ok(true) => converted += 1,
            Ok(false) => skipped += 1,
            Err(err) => {
                failed += 1;
                eprintln!("FAILED {}: {err:#}", path.display());
            }
        }
    }

    println!(
        "{} files: {converted} converted, {skipped} already XML/empty, {failed} failed{}",
        targets.len(),
        if dry_run { " (dry run)" } else { "" }
    );
    if failed > 0 {
        std::process::exit(1);
    }
}

fn convert_file(path: &Path, dry_run: bool) -> anyhow::Result<bool> {
    if !dry_run && repair_scheme_file_format(path)? {
        return Ok(true);
    }

    let raw = std::fs::read_to_string(path)?;
    let trimmed = raw.trim_start();
    if trimmed.is_empty() {
        return Ok(false);
    }
    if trimmed.starts_with("<?xml") || trimmed.starts_with("<scheme") {
        return Ok(false); // already converted
    }
    let id = scheme_id_from_path(path);
    let items = decode_legacy_markdown_items(&raw, id)?;
    let scheme = Scheme {
        id,
        name: String::new(),
        color_index: 0,
        gsync: false,
        source: Default::default(),
        items,
    };
    let xml = encode_scheme_to_xml(&scheme)?;
    // Round-trip the freshly encoded XML back through the loader and confirm the
    // item tree survived before overwriting the original markdown.
    let reparsed = decode_xml_items(&xml, id)?;
    if reparsed.len() != scheme.items.len() {
        anyhow::bail!(
            "round-trip item count mismatch: {} markdown vs {} xml",
            scheme.items.len(),
            reparsed.len()
        );
    }
    for (a, b) in scheme.items.iter().zip(&reparsed) {
        if a.content != b.content {
            anyhow::bail!("round-trip content mismatch on item {}", a.id);
        }
    }
    if !dry_run {
        std::fs::write(path, xml)?;
    }
    Ok(true)
}

/// The scheme id is embedded only informationally in the XML (the loader uses
/// the filename / workspace index), so a date-named daily-queue file can use a
/// fresh id without affecting correctness.
fn scheme_id_from_path(path: &Path) -> SchemeId {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<SchemeId>().ok())
        .unwrap_or_else(SchemeId::new)
}

fn collect_knotq_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_knotq_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("knotq") {
            out.push(path);
        }
    }
}
