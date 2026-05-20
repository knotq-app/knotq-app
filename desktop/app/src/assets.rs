use std::{borrow::Cow, fs, io, path::PathBuf};

use anyhow::Result;
use gpui::{AssetSource, SharedString};

pub struct AppAssets {
    roots: Vec<PathBuf>,
}

impl AppAssets {
    pub fn new() -> Self {
        let installed_assets = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(|parent| parent.join("assets")));
        let source_assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
        let mut roots = Vec::new();
        if let Some(path) = installed_assets {
            roots.push(path);
        }
        roots.push(source_assets);

        Self { roots }
    }
}

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        for root in &self.roots {
            match fs::read(root.join(path)) {
                Ok(data) => return Ok(Some(Cow::Owned(data))),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        for root in &self.roots {
            match fs::read_dir(root.join(path)) {
                Ok(entries) => {
                    return Ok(entries
                        .filter_map(|entry| {
                            entry
                                .ok()
                                .and_then(|entry| entry.file_name().into_string().ok())
                                .map(SharedString::from)
                        })
                        .collect())
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(Vec::new())
    }
}
