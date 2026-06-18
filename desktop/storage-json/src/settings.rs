use anyhow::{anyhow, Context, Result};
use knotq_model::AppSettings;
use serde::{Deserialize, Serialize};
use std::sync::Once;
use std::{fs, path::Path};

use crate::files::{write_atomic, SETTINGS_SCHEMA_VERSION};
use crate::secrets::{self, GoogleSecret, SyncSecret};

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
    let mut settings = env.settings;
    rehydrate_from_keychain(&mut settings);
    Ok(settings)
}

pub fn save_app_settings(path: &Path, settings: &AppSettings) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok();
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
        std::env::temp_dir().join(format!("knotq-settings-test-{}.json", Uuid::new_v4()))
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
        fs::remove_file(&path).ok();

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
        fs::remove_file(&path).ok();
    }
}
