use super::super::*;

/// Localized "Google Calendar {label} failed" title for the sync-result notice
/// modal. `label` is the internal (English, log-oriented) stage identifier
/// passed by the various sync entry points ("import", "refresh",
/// "background sync"); unrecognized labels fall back to the raw English
/// template rather than panicking.
fn google_calendar_sync_failed_title(label: &str) -> String {
    let key = match label {
        "import" => "google.calendar.import_failed_title",
        "refresh" => "google.calendar.refresh_failed_title",
        "background sync" => "google.calendar.background_sync_failed_title",
        _ => return format!("Google Calendar {label} failed"),
    };
    knotq_l10n::t(key).to_string()
}

struct GoogleSyncResultOptions {
    parent: FolderId,
    create_missing: bool,
    open_first_imported: bool,
    always_notify: bool,
    label: &'static str,
}

impl KnotQApp {
    pub(super) fn finish_google_calendar_import(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cancel_token: Option<Arc<AtomicBool>>,
        cx: &mut Context<Self>,
    ) {
        if !self.finish_google_oauth_task(cancel_token.as_ref()) {
            return;
        }
        self.finish_google_sync_result(
            result,
            GoogleSyncResultOptions {
                parent,
                create_missing: true,
                open_first_imported: true,
                always_notify: true,
                label: "import",
            },
            cx,
        );
    }

    pub(super) fn finish_google_calendar_picker_load(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarPickerLoadResult, String>,
        cx: &mut Context<Self>,
    ) {
        if self
            .google_calendar_picker
            .as_ref()
            .is_none_or(|picker| picker.parent != parent)
        {
            return;
        }

        self.google_calendar_picker_task = None;
        match result {
            Ok(result) => {
                let account_count = result.picker_accounts.len();
                let error_count = result
                    .picker_accounts
                    .iter()
                    .filter(|account| account.error.is_some())
                    .count();
                let accounts_changed = self.upsert_google_accounts(result.accounts);
                if accounts_changed {
                    self.save_app_settings();
                }
                google_oauth_log(format!(
                    "picker.load finish accounts={account_count} errors={error_count}"
                ));
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Loaded {
                        accounts: result.picker_accounts,
                    },
                });
            }
            Err(err) => {
                eprintln!("Google Calendar selector failed: {err}");
                google_oauth_log(format!("picker.load failed: {err}"));
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Error(err),
                });
            }
        }
        cx.notify();
    }

    pub(super) fn finish_google_calendar_scheme_refresh(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cancel_token: Option<Arc<AtomicBool>>,
        cx: &mut Context<Self>,
    ) {
        if !self.finish_google_oauth_task(cancel_token.as_ref()) {
            return;
        }
        self.finish_google_sync_result(
            result,
            GoogleSyncResultOptions {
                parent: self.workspace.root,
                create_missing: false,
                open_first_imported: false,
                always_notify: true,
                label: "refresh",
            },
            cx,
        );
    }

    pub(super) fn finish_google_calendar_background_sync(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cx: &mut Context<Self>,
    ) {
        self.finish_google_sync_result(
            result,
            GoogleSyncResultOptions {
                parent: self.workspace.root,
                create_missing: false,
                open_first_imported: false,
                always_notify: false,
                label: "background sync",
            },
            cx,
        );
    }

    fn finish_google_sync_result(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        options: GoogleSyncResultOptions,
        cx: &mut Context<Self>,
    ) {
        let GoogleSyncResultOptions {
            parent,
            create_missing,
            open_first_imported,
            always_notify,
            label,
        } = options;
        match result {
            Ok(result) => {
                let imported_count = result.calendars.len();
                let failure_count = result.failures.len();
                let accounts_changed = self.upsert_google_accounts(result.accounts);
                let applied = self.apply_imported_google_calendars(
                    parent,
                    result.calendars,
                    create_missing,
                    open_first_imported,
                    cx,
                );
                if accounts_changed || create_missing {
                    self.save_app_settings();
                }
                if !result.failures.is_empty() {
                    let failures = result.failures;
                    google_oauth_log(format!(
                        "{label}.finish partial_failure imported={imported_count} failures={failure_count}: {}",
                        failures.join(" | ")
                    ));
                    for failure in &failures {
                        eprintln!("Google Calendar {label} failed: {failure}");
                    }
                    if always_notify {
                        self.show_google_calendar_error(
                            google_calendar_sync_failed_title(label),
                            failures.join("\n"),
                        );
                        self.google_oauth_status = GoogleOAuthStatus::Error;
                    }
                } else if always_notify {
                    self.google_oauth_status = GoogleOAuthStatus::Idle;
                }
                if failure_count == 0 {
                    google_oauth_log(format!(
                        "{label}.finish ok imported={imported_count} content_changed={}",
                        applied.content_changed
                    ));
                }
                if applied.content_changed {
                    self.reschedule_notifications();
                }
                if always_notify || applied.content_changed {
                    cx.notify();
                }
            }
            Err(err) => {
                eprintln!("Google Calendar {label} failed: {err}");
                google_oauth_log(format!("{label}.finish failed: {err}"));
                if always_notify {
                    if is_google_oauth_browser_cancel_or_timeout(&err) {
                        self.google_oauth_status = GoogleOAuthStatus::Idle;
                    } else {
                        self.show_google_calendar_error(
                            google_calendar_sync_failed_title(label),
                            err.clone(),
                        );
                        self.google_oauth_status = GoogleOAuthStatus::Error;
                    }
                    cx.notify();
                }
            }
        }
    }

    pub(super) fn show_google_calendar_error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.notice_modal = Some(NoticeModal {
            title: title.into(),
            message: message.into(),
            button_label: knotq_l10n::t("common.ok").to_string(),
        });
    }

    fn upsert_google_accounts(&mut self, accounts: Vec<GoogleOAuthAccount>) -> bool {
        let mut changed = false;
        for account in accounts {
            if let Some(existing) = self
                .settings
                .google_accounts
                .iter_mut()
                .find(|existing| existing.account_id == account.account_id)
            {
                if existing != &account {
                    *existing = account;
                    changed = true;
                }
            } else {
                self.settings.google_accounts.push(account);
                changed = true;
            }
        }
        changed
    }

    fn apply_imported_google_calendars(
        &mut self,
        parent: FolderId,
        calendars: Vec<ImportedGoogleCalendar>,
        create_missing: bool,
        open_first_imported: bool,
        cx: &mut Context<Self>,
    ) -> GoogleCalendarApplyResult {
        let parent = if self.workspace.folder(parent).is_some() {
            parent
        } else {
            self.workspace.root
        };
        if parent != self.workspace.root {
            if let Some(folder) = self.workspace.folders.get_mut(&parent) {
                folder.expanded = true;
            }
        }

        let mut first_scheme = None;
        let mut synced = 0usize;
        let mut content_changed = false;
        for calendar in calendars {
            let existing_scheme_ids = active_google_calendar_scheme_ids(&self.workspace, &calendar);
            let existing_scheme_id = existing_scheme_ids
                .first()
                .copied()
                .or_else(|| find_google_calendar_scheme(&self.workspace, &calendar));
            if self.delete_duplicate_google_calendar_schemes(
                existing_scheme_ids.get(1..).unwrap_or(&[]),
                cx,
            ) {
                content_changed = true;
            }
            let scheme_id = match existing_scheme_id {
                Some(scheme_id) => {
                    if create_missing && self.workspace.is_scheme_deleted(scheme_id) {
                        self.restore_deleted_scheme(scheme_id, cx);
                        content_changed = true;
                    }
                    scheme_id
                }
                None if create_missing => {
                    let color_index = calendar.color_index;
                    let mut scheme = Scheme::new(calendar.name.clone(), color_index);
                    let id = scheme.id;
                    scheme.source = google_calendar_source(&calendar);
                    self.workspace.schemes.insert(id, scheme);
                    if let Some(folder) = self.workspace.folders.get_mut(&parent) {
                        folder.children.push(NodeRef::Scheme(id));
                    }
                    content_changed = true;
                    id
                }
                None => continue,
            };

            if let Some(scheme) = self.workspace.schemes.get_mut(&scheme_id) {
                let should_update_name = existing_scheme_id.is_none();
                let metadata_changed =
                    apply_google_calendar_metadata(scheme, &calendar, should_update_name);
                let items_changed = apply_google_calendar_items(scheme, &calendar);
                self.state.mark_scheme_dirty(scheme_id);
                let scheme_content_changed = metadata_changed || items_changed;
                if scheme_content_changed {
                    content_changed = true;
                    if self
                        .scheme_editor
                        .as_ref()
                        .is_some_and(|(id, _)| *id == scheme_id)
                    {
                        self.scheme_editor = None;
                        self._editor_subscription = None;
                    }
                    self.scheme_sessions.remove(&scheme_id);
                }
                first_scheme.get_or_insert(scheme_id);
                synced += 1;
            }
        }

        if synced > 0 {
            self.reconcile_workspace_ui_state();
            self.state.mark_index_dirty();
            if open_first_imported {
                if let Some(scheme_id) = first_scheme {
                    self.open_scheme(scheme_id, None);
                }
            }
        }
        GoogleCalendarApplyResult { content_changed }
    }

    fn delete_duplicate_google_calendar_schemes(
        &mut self,
        scheme_ids: &[SchemeId],
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        for scheme_id in scheme_ids.iter().copied() {
            if self.workspace.is_scheme_deleted(scheme_id)
                || !self.workspace.schemes.contains_key(&scheme_id)
            {
                continue;
            }

            let was_selected = self.selection.scheme_id == Some(scheme_id);
            let fallback = was_selected
                .then(|| self.first_visible_scheme_id_except(scheme_id))
                .flatten();

            let mut origin = None;
            for (folder_id, folder) in self.workspace.folders.iter_mut() {
                let mut index = 0usize;
                while index < folder.children.len() {
                    if folder.children[index] == NodeRef::Scheme(scheme_id) {
                        if origin.is_none() {
                            origin = Some((*folder_id, index));
                        }
                        folder.children.remove(index);
                    } else {
                        index += 1;
                    }
                }
            }

            if let Some((folder_id, position)) = origin {
                self.workspace
                    .mark_scheme_deleted_from(scheme_id, folder_id, position);
            } else {
                self.workspace.mark_scheme_deleted(scheme_id);
            }
            self.trash_expanded = true;
            self.state.mark_index_dirty();
            if self
                .scheme_editor
                .as_ref()
                .is_some_and(|(id, _)| *id == scheme_id)
            {
                self.scheme_editor = None;
                self._editor_subscription = None;
            }
            self.close_popovers_for_scheme(scheme_id);
            self.scheme_sessions.remove(&scheme_id);
            if was_selected {
                if let Some(next_id) = fallback {
                    self.open_scheme(next_id, None);
                } else {
                    self.open_union();
                    self.selection.scheme_id = None;
                    self.selection.focused_item_id = None;
                }
                cx.notify();
            }
            changed = true;
        }
        changed
    }
}
