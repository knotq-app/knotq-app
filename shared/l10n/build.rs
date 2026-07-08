use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Embeds every catalog in `app/l10n/*.json` so the runtime never touches the
/// filesystem. Adding a locale file is picked up automatically on rebuild.
fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let l10n_dir = manifest_dir.join("../../l10n").canonicalize().unwrap();
    println!("cargo:rerun-if-changed={}", l10n_dir.display());

    let mut codes = Vec::new();
    for entry in fs::read_dir(&l10n_dir).unwrap() {
        let path = entry.unwrap().path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") || name == "locales.json" {
            continue;
        }
        codes.push(name.trim_end_matches(".json").to_string());
    }
    codes.sort();

    let mut out = String::new();
    writeln!(
        out,
        "pub const LOCALES_JSON: &str = include_str!(r\"{}\");",
        l10n_dir.join("locales.json").display()
    )
    .unwrap();
    writeln!(out, "pub const CATALOGS: &[(&str, &str)] = &[").unwrap();
    for code in &codes {
        writeln!(
            out,
            "    ({code:?}, include_str!(r\"{}\")),",
            l10n_dir.join(format!("{code}.json")).display()
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();

    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("embedded_catalogs.rs");
    fs::write(dest, out).unwrap();
}
