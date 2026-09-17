use super::super::*;
use knotq_commands::Command;

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

/// The commands that bring an imported calendar scheme from `current` to
/// `updated`: name, source, and the full item list. Items are replaced by
/// deleting every current row and inserting every updated one — ids are kept,
/// and sync diffs the resulting state, so an unchanged row costs nothing.
fn imported_scheme_update_command(current: &Scheme, updated: Scheme) -> Option<Command> {
    let id = current.id;
    let mut commands = Vec::new();
    if updated.name != current.name {
        commands.push(Command::RenameScheme {
            id,
            name: updated.name,
        });
    }
    if updated.source != current.source {
        commands.push(Command::SetSchemeSource {
            id,
            source: updated.source,
        });
    }
    if updated.items != current.items {
        commands.extend(current.items.iter().map(|item| Command::DeleteItem {
            scheme: id,
            item: item.id,
        }));
        commands.extend(
            updated
                .items
                .into_iter()
                .enumerate()
                .map(|(position, item)| Command::InsertItem {
                    scheme: id,
                    position,
                    item,
                }),
        );
    }
    Command::from_vec(commands)
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
                    if is_google_oauth_user_cancelled(&err) {
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

    /// Apply an import-driven change through the store. Importer origin: an
    /// import legitimately writes the read-only calendar schemes a user cannot.
    /// Not undoable. Returns whether it applied.
    fn apply_import_command(&mut self, command: Command) -> bool {
        match self
            .state
            .apply_prechecked_local_command(command, knotq_commands::CommandOrigin::Importer)
        {
            Ok(_) => true,
            Err(err) => {
                eprintln!("Google Calendar import command failed: {err}");
                false
            }
        }
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
        if parent != self.workspace.root
            && self
                .workspace
                .folder(parent)
                .is_some_and(|folder| !folder.expanded)
        {
            self.apply_import_command(Command::SetFolderExpanded {
                id: parent,
                expanded: true,
            });
        }

        let mut first_scheme = None;
        let mut synced = 0usize;
        let mut content_changed = false;
        for calendar in calendars {
            if calendar.calendar_deleted {
                let stale_scheme_ids =
                    active_google_calendar_scheme_ids(&self.workspace, &calendar);
                if self.delete_duplicate_google_calendar_schemes(&stale_scheme_ids, cx) {
                    content_changed = true;
                }
                continue;
            }
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
                    let position = self
                        .workspace
                        .folder(parent)
                        .map_or(0, |folder| folder.children.len());
                    if !self.apply_import_command(Command::RestoreScheme {
                        folder: parent,
                        position,
                        scheme,
                    }) {
                        continue;
                    }
                    content_changed = true;
                    id
                }
                None => continue,
            };

            if let Some(current) = self.workspace.scheme(scheme_id).cloned() {
                let should_update_name = existing_scheme_id.is_none();
                let mut updated = current.clone();
                let metadata_changed =
                    apply_google_calendar_metadata(&mut updated, &calendar, should_update_name);
                let items_changed = apply_google_calendar_items(&mut updated, &calendar);
                let scheme_content_changed = metadata_changed || items_changed;
                if let Some(command) = imported_scheme_update_command(&current, updated) {
                    self.apply_import_command(command);
                }
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

            if !self.apply_import_command(Command::DeleteScheme { id: scheme_id }) {
                continue;
            }
            self.trash_expanded = true;
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
