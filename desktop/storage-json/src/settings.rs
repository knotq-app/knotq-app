use anyhow::{anyhow, Context, Result};
use knotq_model::AppSettings;
use serde::{Deserialize, Serialize};
use std::sync::Once;
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::files::{write_atomic, SETTINGS_SCHEMA_VERSION};
use crate::secrets::{self, GoogleSecret, SyncSecret};

/// Just enough of the envelope to decide whether this build may read the rest.
#[derive(Deserialize)]
struct SettingsVersion {
    #[serde(default)]
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct SettingsEnvelope {
    version: u32,
    settings: AppSettings,
}

/// Why a settings file could not be loaded. The two cases call for opposite
/// responses, which is the whole reason this is not a bare `anyhow::Error`:
/// a file from a newer build must be left exactly as it is, while an unreadable
/// one has to be moved out of the way before this build can save at all.
#[derive(Debug)]
pub enum SettingsLoadError {
    /// Written by a build that knows a later schema. Its extra keys are
    /// meaningful and this build would drop them, so it must not be rewritten.
    TooNew { found: u32, supported: u32 },
    /// Missing, unreadable, or not parseable as settings.
    Unreadable(anyhow::Error),
}

impl std::fmt::Display for SettingsLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooNew { found, supported } => write!(
                f,
                "settings.json was written by a newer version of KnotQ \
                 (schema {found}, this build understands {supported})"
            ),
            Self::Unreadable(err) => write!(f, "{err:#}"),
        }
    }
}

impl From<SettingsLoadError> for anyhow::Error {
    fn from(err: SettingsLoadError) -> Self {
        match err {
            SettingsLoadError::Unreadable(err) => err,
            other => anyhow!("{other}"),
        }
    }
}

/// What startup should do about the settings file.
pub struct SettingsBootstrap {
    pub settings: AppSettings,
    /// When set, this session must not write `settings.json`: the file on disk
    /// holds something this build cannot represent, and saving would replace it
    /// with a lossy version. The reason is meant to be shown and logged.
    pub save_blocked_reason: Option<String>,
    /// Where an unreadable file was moved, if one was.
    pub quarantined: Option<PathBuf>,
}

/// Load the settings, recovering rather than resetting.
///
/// A settings file is small but it is the only record of *who this device is*:
/// the sync account, the linked Google accounts, the window, the theme. The
/// previous behaviour — return `Err`, let the caller fall back to
/// `AppSettings::default()`, and then overwrite the file on the next save —
/// turned any single unreadable byte into a silent sign-out with the evidence
/// destroyed. Neither outcome here loses the original: a file from a newer build
/// is left alone (and saving is blocked until that build runs again), and an
/// unreadable one is moved aside where support can ask for it.
pub fn load_settings_or_recover(path: &Path) -> SettingsBootstrap {
    match load_settings_detailed(path) {
        Ok(settings) => SettingsBootstrap {
            settings,
            save_blocked_reason: None,
            quarantined: None,
        },
        Err(SettingsLoadError::TooNew { found, supported }) => {
            let reason = SettingsLoadError::TooNew { found, supported }.to_string();
            eprintln!("{reason}; running without saving settings");
            SettingsBootstrap {
                settings: AppSettings::default(),
                save_blocked_reason: Some(reason),
                quarantined: None,
            }
        }
        Err(SettingsLoadError::Unreadable(err)) => match quarantine(path) {
            Ok(moved) => {
                eprintln!(
                    "settings could not be read ({err:#}); the file was kept as {} and this \
                     session starts from defaults",
                    moved.display()
                );
                SettingsBootstrap {
                    settings: AppSettings::default(),
                    save_blocked_reason: None,
                    quarantined: Some(moved),
                }
            }
            // Could not move it aside, so we must not write over it either.
            Err(move_err) => {
                let reason = format!(
                    "settings unreadable ({err:#}) and could not be set aside ({move_err:#})"
                );
                eprintln!("{reason}; running without saving settings");
                SettingsBootstrap {
                    settings: AppSettings::default(),
                    save_blocked_reason: Some(reason),
                    quarantined: None,
                }
            }
        },
    }
}

/// Rename the unreadable file to a timestamped sibling. Never deletes: the file
/// is the user's only copy of their account link, and a later build (or a human)
/// may still be able to read it.
fn quarantine(path: &Path) -> Result<PathBuf> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "settings.json".to_string());
    let moved = path.with_file_name(format!("{name}.unreadable-{stamp}"));
    fs::rename(path, &moved)
        .with_context(|| format!("move {} to {}", path.display(), moved.display()))?;
    Ok(moved)
}

pub fn load_app_settings(path: &Path) -> Result<AppSettings> {
    load_settings_detailed(path).map_err(anyhow::Error::from)
}

fn load_settings_detailed(path: &Path) -> std::result::Result<AppSettings, SettingsLoadError> {
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))
        .map_err(SettingsLoadError::Unreadable)?;
    if raw.trim().is_empty() {
        return Ok(AppSettings::default());
    }
    // The version has to be read *before* the body: a file from a newer build is
    // expected to hold a body this one cannot deserialize, and parsing first
    // would report the most important case — do not touch this file — as
    // ordinary corruption, and move the user's account link out of the way.
    let probe: SettingsVersion = serde_json::from_str(&raw)
        .context("parse settings.json")
        .map_err(SettingsLoadError::Unreadable)?;
    if probe.version > SETTINGS_SCHEMA_VERSION {
        return Err(SettingsLoadError::TooNew {
            found: probe.version,
            supported: SETTINGS_SCHEMA_VERSION,
        });
    }
    // An older file is expected and fine: every field carries `#[serde(default)]`,
    // so keys added since are filled with the defaults the feature shipped with —
    // `tests/upgrade_from_release.rs` holds that line against real released files.
    let env: SettingsEnvelope = serde_json::from_str(&raw)
        .context("parse settings.json")
        .map_err(SettingsLoadError::Unreadable)?;
    let mut settings = env.settings;
    rehydrate_from_keychain(&mut settings);
    Ok(settings)
}

pub fn save_app_settings(path: &Path, settings: &AppSettings) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
        secure_directory_permissions(dir)?;
    }
    // Work on a clone so the live in-memory settings keep their tokens; only the
    // serialized copy is redacted.
    let mut to_write = settings.clone();
    redact_into_keychain(&mut to_write);
    let env = SettingsEnvelope {
        version: SETTINGS_SCHEMA_VERSION,
        settings: to_write,
    };
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic(path, json.as_bytes())?;
    secure_file_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn secure_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("secure {}", path.display()))
}

#[cfg(not(unix))]
fn secure_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn secure_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("secure {}", path.display()))
}

#[cfg(not(unix))]
fn secure_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Move secret tokens out of `settings` into the OS keychain, blanking the
/// in-memory copy that is about to be serialized. A holder whose keychain write
/// fails keeps its plaintext secrets in the JSON so a session is never lost.
fn redact_into_keychain(settings: &mut AppSettings) {
    if !secrets::is_enabled() {
        return;
    }
    if let Some(account) = settings.sync_account.as_mut() {
        if !account.bearer_token.is_empty() || account.refresh_token.is_some() {
            let secret = SyncSecret {
                bearer: account.bearer_token.clone(),
                refresh: account.refresh_token.clone(),
            };
            match secrets::store_sync(&secret) {
                Ok(()) => {
                    account.bearer_token.clear();
                    account.refresh_token = None;
                }
                Err(err) => warn_keychain_unavailable("sync token", &err),
            }
        }
    }
    for account in settings.google_accounts.iter_mut() {
        if account.access_token.is_empty() && account.refresh_token.is_empty() {
            continue;
        }
        let secret = GoogleSecret {
            access: account.access_token.clone(),
            refresh: account.refresh_token.clone(),
        };
        match secrets::store_google(&account.account_id, &secret) {
            Ok(()) => {
                account.access_token.clear();
                account.refresh_token.clear();
            }
            Err(err) => warn_keychain_unavailable("Google token", &err),
        }
    }
}

/// Pull secret tokens from the keychain into redacted settings.
fn rehydrate_from_keychain(settings: &mut AppSettings) {
    if !secrets::is_enabled() {
        return;
    }
    if let Some(account) = settings.sync_account.as_mut() {
        if account.bearer_token.is_empty() && account.refresh_token.is_none() {
            // Redacted on disk — load the real tokens from the keychain.
            if let Ok(secret) = secrets::load_sync() {
                account.bearer_token = secret.bearer;
                account.refresh_token = secret.refresh;
            }
        }
    }
    for account in settings.google_accounts.iter_mut() {
        if account.access_token.is_empty() && account.refresh_token.is_empty() {
            if let Ok(secret) = secrets::load_google(&account.account_id) {
                account.access_token = secret.access;
                account.refresh_token = secret.refresh;
            }
        }
    }
}

/// Warn (once per process) that the OS keychain could not be reached, so we are
/// falling back to keeping auth tokens in `settings.json`.
fn warn_keychain_unavailable(context: &str, err: &anyhow::Error) {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "KnotQ: OS keychain unavailable ({context}: {err}); keeping auth tokens in settings.json"
        );
    });
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use knotq_model::{CalendarWeekRange, GoogleOAuthAccount, SyncAccountSettings, ThemeMode};
    use uuid::Uuid;

    use super::*;

    fn temp_settings_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("knotq-settings-test-{}", Uuid::new_v4()))
            .join("settings.json")
    }

    fn sync_account(bearer: &str, refresh: &str) -> SyncAccountSettings {
        SyncAccountSettings {
            api_base: "https://sync.example.com".into(),
            user_id: "11111111-1111-1111-1111-111111111111".into(),
            session_id: Some("22222222-2222-2222-2222-222222222222".into()),
            workspace_id: Some("33333333-3333-3333-3333-333333333333".into()),
            email: "user@example.com".into(),
            supports_sync: true,
            bearer_token: bearer.into(),
            expires_at: Utc::now(),
            refresh_token: Some(refresh.into()),
            refresh_expires_at: None,
            account_status: None,
        }
    }

    fn google_account(id: &str, access: &str, refresh: &str) -> GoogleOAuthAccount {
        GoogleOAuthAccount {
            account_id: id.into(),
            email: Some("user@example.com".into()),
            client_id: "client-id".into(),
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at: None,
            scope: "calendar".into(),
            token_source: knotq_model::GoogleTokenSource::OAuthRefreshToken,
            needs_reauth: false,
        }
    }

    #[test]
    fn app_settings_default_to_system_theme() {
        assert_eq!(AppSettings::default().theme_mode, ThemeMode::System);
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

    /// The full secret lifecycle, kept in a single test so the process-global
    /// `KNOTQ_DISABLE_KEYCHAIN` env var is mutated sequentially (never racing
    /// another test). Covers: redact-on-save + rehydrate-on-load, and the
    /// disable-keychain fallback.
    #[test]
    fn keychain_secret_lifecycle() {
        std::env::remove_var("KNOTQ_DISABLE_KEYCHAIN");

        // 1. Save redacts the on-disk copy; load rehydrates from the keychain.
        let path = temp_settings_path();
        let settings = AppSettings {
            sync_account: Some(sync_account("BEARER-SECRET", "REFRESH-SECRET")),
            google_accounts: vec![google_account("acct-1", "GOOGLE-ACCESS", "GOOGLE-REFRESH")],
            ..Default::default()
        };
        save_app_settings(&path, &settings).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("BEARER-SECRET"),
            "bearer token leaked to disk"
        );
        assert!(
            !raw.contains("REFRESH-SECRET"),
            "refresh token leaked to disk"
        );
        assert!(
            !raw.contains("GOOGLE-ACCESS"),
            "google access leaked to disk"
        );
        assert!(
            !raw.contains("GOOGLE-REFRESH"),
            "google refresh leaked to disk"
        );

        let loaded = load_app_settings(&path).unwrap();
        let account = loaded.sync_account.unwrap();
        assert_eq!(account.bearer_token, "BEARER-SECRET");
        assert_eq!(account.refresh_token.as_deref(), Some("REFRESH-SECRET"));
        let g = &loaded.google_accounts[0];
        assert_eq!(g.access_token, "GOOGLE-ACCESS");
        assert_eq!(g.refresh_token, "GOOGLE-REFRESH");
        fs::remove_dir_all(path.parent().unwrap()).ok();

        // 2. With the keychain disabled, tokens stay in the file and round-trip.
        let path = temp_settings_path();
        std::env::set_var("KNOTQ_DISABLE_KEYCHAIN", "1");
        let settings = AppSettings {
            sync_account: Some(sync_account("INFILE-BEARER", "INFILE-REFRESH")),
            ..Default::default()
        };
        save_app_settings(&path, &settings).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("INFILE-BEARER"),
            "disabled keychain keeps token in file"
        );
        let loaded = load_app_settings(&path).unwrap();
        assert_eq!(loaded.sync_account.unwrap().bearer_token, "INFILE-BEARER");
        std::env::remove_var("KNOTQ_DISABLE_KEYCHAIN");
        fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}
