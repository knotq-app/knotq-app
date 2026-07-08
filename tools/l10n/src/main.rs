//! Catalog tooling for KnotQ localization: merges extraction partials into the
//! canonical `en.json`, validates every locale against it, and regenerates the
//! platform artifacts (iOS `.xcstrings`, Android `strings.xml` + `L10n.kt`,
//! website locale scripts). See `app/l10n/README.md` for the format.

mod catalog;
mod gen_android;
mod gen_ios;
mod gen_website;
mod merge;
mod validate;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn app_root() -> PathBuf {
    // tools/l10n → app repo root, independent of the caller's cwd.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/l10n has an app root two levels up")
        .to_path_buf()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = app_root();
    let l10n_dir = root.join("l10n");

    match args.first().map(String::as_str) {
        Some("merge") => merge::run(&l10n_dir),
        Some("validate") => validate::run(&l10n_dir),
        Some("generate") => {
            let targets: Vec<&str> = args
                .get(1)
                .map(|t| t.split(',').collect())
                .unwrap_or_else(|| vec!["ios", "android", "website"]);
            // Never generate from a broken catalog.
            validate::run(&l10n_dir)?;
            let catalogs = catalog::load_all(&l10n_dir)?;
            let config = catalog::TargetConfig::load(&l10n_dir, &root)?;
            for target in targets {
                match target {
                    "ios" => gen_ios::run(&catalogs, &config)?,
                    "android" => gen_android::run(&catalogs, &config)?,
                    "website" => gen_website::run(&catalogs, &config)?,
                    other => bail!("unknown generate target {other:?}"),
                }
            }
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: l10n-gen <merge | validate | generate [ios,android,website]>\n\
                 \n\
                 merge     fold l10n/partial/*.json into en.json (then delete the partials)\n\
                 validate  check every locale catalog against en.json\n\
                 generate  validate, then rewrite platform artifacts"
            );
            bail!("missing or unknown subcommand");
        }
    }
    .context("l10n-gen failed")
}
