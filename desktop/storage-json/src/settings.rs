use anyhow::{anyhow, Context, Result};
use knotq_model::AppSettings;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

use crate::files::{write_atomic, SETTINGS_SCHEMA_VERSION};

#[derive(Serialize, Deserialize)]
struct SettingsEnvelope {
    version: u32,
    settings: AppSettings,
}

pub fn load_app_settings(path: &Path) -> Result<AppSettings> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(AppSettings::default());
    }
    let env: SettingsEnvelope = serde_json::from_str(&raw).context("parse settings.json")?;
    if env.version != SETTINGS_SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported settings schema version {}, expected {}",
            env.version,
            SETTINGS_SCHEMA_VERSION
        ));
    }
    Ok(env.settings)
}

pub fn save_app_settings(path: &Path, settings: &AppSettings) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
    }
    let env = SettingsEnvelope {
        version: SETTINGS_SCHEMA_VERSION,
        settings: settings.clone(),
    };
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic(path, json.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use knotq_model::{CalendarWeekRange, ThemeMode};

    use super::*;

    #[test]
    fn app_settings_default_to_dark_theme() {
        assert_eq!(AppSettings::default().theme_mode, ThemeMode::Dark);
        assert_eq!(
            AppSettings::default().calendar_week_range,
            CalendarWeekRange::NextSevenDays
        );
        assert_eq!(
            AppSettings::default()
                .notification_defaults
                .event_offset_secs,
            10 * 60
        );
        assert_eq!(
            AppSettings::default()
                .notification_defaults
                .assignment_offset_secs,
            2 * 60 * 60
        );
        assert!(AppSettings::default().auto_update);
    }
}
