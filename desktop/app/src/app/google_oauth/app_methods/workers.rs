use super::super::*;

pub(super) fn run_google_calendar_picker_load(
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
) -> Result<GoogleCalendarPickerLoadResult> {
    let mut updated_accounts = Vec::new();
    let mut picker_accounts = Vec::new();

    for mut account in accounts {
        let label = google_account_label(&account).to_string();

        if let Err(err) = refresh_google_access_token_if_needed(&config, &mut account) {
            google_oauth_log(format!("picker.refresh failed account={label}: {err:#}"));
            picker_accounts.push(GoogleCalendarPickerAccount {
                account_id: account.account_id.clone(),
                label,
                calendars: Vec::new(),
                error: Some(format!("{err:#}")),
            });
            updated_accounts.push(account);
            continue;
        }

        match list_google_calendars(&account.access_token) {
            Ok(calendars) => {
                google_oauth_log(format!(
                    "picker.calendar_list ok account={label} calendars={}",
                    calendars.len()
                ));
                let calendars = calendars
                    .into_iter()
                    .map(|calendar| {
                        let already_added = existing_sources.iter().any(|source| {
                            source.calendar_id == calendar.id
                                && existing_source_matches_google_account(source, &account)
                        });
                        GoogleCalendarPickerCalendar {
                            id: calendar.id.clone(),
                            label: google_calendar_name(&calendar),
                            already_added,
                        }
                    })
                    .collect();
                picker_accounts.push(GoogleCalendarPickerAccount {
                    account_id: account.account_id.clone(),
                    label,
                    calendars,
                    error: None,
                });
            }
            Err(err) => {
                google_oauth_log(format!(
                    "picker.calendar_list failed account={label}: {err:#}"
                ));
                picker_accounts.push(GoogleCalendarPickerAccount {
                    account_id: account.account_id.clone(),
                    label,
                    calendars: Vec::new(),
                    error: Some(format!("{err:#}")),
                });
            }
        }
        updated_accounts.push(account);
    }

    Ok(GoogleCalendarPickerLoadResult {
        accounts: updated_accounts,
        picker_accounts,
    })
}

pub(super) fn run_google_calendar_import(
    config: GoogleOAuthConfig,
    _existing_accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
    cancel_token: Arc<AtomicBool>,
) -> Result<GoogleCalendarImportResult> {
    google_oauth_log("import.oauth start");
    let accounts = vec![run_google_oauth(config.clone(), &cancel_token)?];
    run_google_calendar_sync(
        config,
        accounts,
        existing_sources,
        GoogleCalendarImportMode::MissingOnly,
        None,
    )
}

pub(super) fn run_google_calendar_import_existing_account_calendar(
    config: GoogleOAuthConfig,
    account: GoogleOAuthAccount,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
    calendar_id: String,
) -> Result<GoogleCalendarImportResult> {
    google_oauth_log(format!(
        "import_existing_account start account={} calendar_id={calendar_id}",
        google_account_label(&account)
    ));
    run_google_calendar_sync(
        config,
        vec![account],
        existing_sources,
        GoogleCalendarImportMode::MissingOnly,
        Some(calendar_id),
    )
}

pub(super) fn run_google_calendar_scheme_reconnect(
    config: GoogleOAuthConfig,
    source: ImportedCalendarSource,
    cancel_token: Arc<AtomicBool>,
) -> Result<GoogleCalendarImportResult> {
    google_oauth_log(format!(
        "reconnect.oauth start account={} calendar_id={}",
        google_calendar_source_target_label(&source),
        source.calendar_id
    ));
    let account = run_google_oauth(config.clone(), &cancel_token)?;
    if !google_account_matches_calendar_source(&account, &source) {
        let signed_in = account
            .email
            .clone()
            .unwrap_or_else(|| account.account_id.clone());
        google_oauth_log(format!(
            "reconnect.oauth account_mismatch signed_in={signed_in} expected={}",
            google_calendar_source_target_label(&source)
        ));
        bail!(
            "signed in as {signed_in}, but this calendar belongs to {}",
            google_calendar_source_target_label(&source)
        );
    }
    run_google_calendar_sync(
        config,
        vec![account],
        vec![ExistingGoogleCalendarSource {
            account_id: source.account_id,
            account_email: source.account_email,
            calendar_id: source.calendar_id,
            sync_token: source.sync_token,
        }],
        GoogleCalendarImportMode::ExistingOnly,
        None,
    )
}

pub(super) fn run_google_calendar_background_sync(
    config: GoogleOAuthConfig,
    existing_accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
) -> Result<GoogleCalendarImportResult> {
    if existing_accounts.is_empty() || existing_sources.is_empty() {
        google_oauth_log(format!(
            "background_sync skipped accounts={} sources={}",
            existing_accounts.len(),
            existing_sources.len()
        ));
        return Ok(GoogleCalendarImportResult {
            accounts: Vec::new(),
            calendars: Vec::new(),
            failures: Vec::new(),
        });
    }
    run_google_calendar_sync(
        config,
        existing_accounts,
        existing_sources,
        GoogleCalendarImportMode::ExistingOnly,
        None,
    )
}

fn run_google_calendar_sync(
    config: GoogleOAuthConfig,
    accounts: Vec<GoogleOAuthAccount>,
    existing_sources: Vec<ExistingGoogleCalendarSource>,
    mode: GoogleCalendarImportMode,
    target_calendar_id: Option<String>,
) -> Result<GoogleCalendarImportResult> {
    google_oauth_log(format!(
        "sync.start mode={} accounts={} sources={} target_calendar={}",
        google_import_mode_label(mode),
        accounts.len(),
        existing_sources.len(),
        target_calendar_id.as_deref().unwrap_or("<all>")
    ));
    let mut updated_accounts = Vec::new();
    let mut calendars = Vec::new();
    let mut failures = Vec::new();

    for mut account in accounts {
        if let Err(err) = refresh_google_access_token_if_needed(&config, &mut account) {
            google_oauth_log(format!(
                "sync.refresh failed account={}: {err:#}",
                google_account_label(&account)
            ));
            failures.push(format!(
                "{}: {err:#}",
                account.email.as_deref().unwrap_or(&account.account_id)
            ));
            updated_accounts.push(account);
            continue;
        }

        match import_google_account_calendars(
            &account,
            &existing_sources,
            mode,
            target_calendar_id.as_deref(),
        ) {
            Ok((mut imported, mut account_failures)) => {
                calendars.append(&mut imported);
                failures.append(&mut account_failures);
            }
            Err(err) => failures.push(format!(
                "{}: {err:#}",
                account.email.as_deref().unwrap_or(&account.account_id)
            )),
        }
        updated_accounts.push(account);
    }

    google_oauth_log(format!(
        "sync.finish accounts_updated={} calendars_imported={} failures={}",
        updated_accounts.len(),
        calendars.len(),
        failures.len()
    ));
    Ok(GoogleCalendarImportResult {
        accounts: updated_accounts,
        calendars,
        failures,
    })
}
