use super::*;

impl KnotQApp {
    fn begin_google_oauth_browser_flow(&mut self) -> Arc<AtomicBool> {
        self.cancel_google_oauth_browser_flow();
        let cancel_token = Arc::new(AtomicBool::new(false));
        self.google_oauth_cancel_token = Some(cancel_token.clone());
        cancel_token
    }

    fn cancel_google_oauth_browser_flow(&mut self) {
        if let Some(cancel_token) = self.google_oauth_cancel_token.take() {
            google_oauth_log("oauth.cancel requested");
            cancel_token.store(true, Ordering::SeqCst);
            self.google_oauth_task = None;
        }
    }

    fn finish_google_oauth_task(&mut self, cancel_token: Option<&Arc<AtomicBool>>) -> bool {
        if let Some(cancel_token) = cancel_token {
            match self.google_oauth_cancel_token.as_ref() {
                Some(current) if Arc::ptr_eq(current, cancel_token) => {
                    self.google_oauth_cancel_token = None;
                }
                _ => {
                    google_oauth_log("oauth.finish stale ignored");
                    return false;
                }
            }
        }

        self.google_oauth_task = None;
        true
    }

    pub(crate) fn open_google_calendar_picker(
        &mut self,
        parent: FolderId,
        position: gpui::Point<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) {
        google_oauth_log("picker.open requested");
        self.sidebar_context_menu = Some(SidebarContextMenu {
            target: SidebarContextTarget::GoogleCalendarPicker { parent },
            position,
        });
        self.google_calendar_picker = Some(GoogleCalendarPickerState {
            parent,
            status: GoogleCalendarPickerStatus::Loading,
        });

        let accounts = self.settings.google_accounts.clone();
        let sources = active_google_calendar_sources(&self.workspace);
        if accounts.is_empty() {
            google_oauth_log("picker.open no stored Google accounts");
            self.google_calendar_picker = Some(GoogleCalendarPickerState {
                parent,
                status: GoogleCalendarPickerStatus::Loaded {
                    accounts: Vec::new(),
                },
            });
            self.google_calendar_picker_task = None;
            cx.notify();
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(&accounts) {
            Ok(config) => config,
            Err(err) => {
                google_oauth_log(format!("picker.open disabled: {err:#}"));
                self.google_calendar_picker = Some(GoogleCalendarPickerState {
                    parent,
                    status: GoogleCalendarPickerStatus::Error(format!("{err:#}")),
                });
                self.google_calendar_picker_task = None;
                cx.notify();
                return;
            }
        };

        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    google_oauth_log(format!(
                        "picker.load start accounts={} existing_sources={}",
                        accounts.len(),
                        sources.len()
                    ));
                    let result = run_google_calendar_picker_load(config, accounts, sources)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_picker_load(parent, result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_picker_load(
                                    parent,
                                    Err("Google Calendar selector worker stopped".to_string()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_calendar_picker_task = Some(task);
        cx.notify();
    }

    pub(crate) fn spawn_google_calendar_sync_task(cx: &mut Context<Self>) -> gpui::Task<()> {
        cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| loop {
                cx.background_executor()
                    .timer(StdDuration::from_secs(
                        GOOGLE_CALENDAR_BACKGROUND_SYNC_INTERVAL_SECS,
                    ))
                    .await;

                let snapshot = weak
                    .update(cx, |app, _cx| app.google_calendar_background_snapshot())
                    .ok()
                    .flatten();
                let Some(snapshot) = snapshot else {
                    continue;
                };

                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    google_oauth_log(format!(
                        "background_sync.worker start accounts={} sources={}",
                        snapshot.accounts.len(),
                        snapshot.sources.len()
                    ));
                    let result = run_google_calendar_background_sync(
                        snapshot.config,
                        snapshot.accounts,
                        snapshot.sources,
                    )
                    .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_background_sync(result, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            eprintln!("background Google Calendar sync worker stopped");
                            break;
                        }
                    }
                }
            },
        )
    }

    pub(crate) fn start_google_calendar_import(
        &mut self,
        parent: FolderId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress)
            && self.google_oauth_cancel_token.is_none()
        {
            return;
        }

        let config = match google_oauth_config_from_build() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar import failed: {err:#}");
                google_oauth_log(format!("import.disabled: {err:#}"));
                self.show_google_calendar_error("Google Calendar import", format!("{err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        let accounts = self.settings.google_accounts.clone();
        let sources = active_google_calendar_sources(&self.workspace);
        google_oauth_log(format!(
            "import.requested stored_accounts={} existing_sources={}",
            accounts.len(),
            sources.len()
        ));

        let cancel_token = self.begin_google_oauth_browser_flow();
        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                let worker_cancel_token = cancel_token.clone();
                let finish_cancel_token = cancel_token.clone();
                std::thread::spawn(move || {
                    google_oauth_log("import.worker start");
                    let result =
                        run_google_calendar_import(config, accounts, sources, worker_cancel_token)
                            .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(
                                    parent,
                                    result,
                                    Some(finish_cancel_token.clone()),
                                    cx,
                                );
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(
                                    parent,
                                    Err("Google OAuth worker stopped".to_string()),
                                    Some(finish_cancel_token.clone()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_import_calendar(
        &mut self,
        parent: FolderId,
        account_id: String,
        calendar_id: String,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let Some(account) = self
            .settings
            .google_accounts
            .iter()
            .find(|account| account.account_id == account_id)
            .cloned()
        else {
            self.show_google_calendar_error(
                "Google Calendar import",
                "That Google account is not connected locally anymore.".to_string(),
            );
            self.google_oauth_status = GoogleOAuthStatus::Error;
            cx.notify();
            return;
        };

        if let Some(scheme_id) =
            find_google_calendar_scheme_for_account(&self.workspace, &account, &calendar_id)
        {
            self.open_scheme(scheme_id, None);
            cx.notify();
            return;
        }
        if let Some(scheme_id) = find_archived_google_calendar_scheme_for_account(
            &self.workspace,
            &account,
            &calendar_id,
        ) {
            self.restore_deleted_scheme(scheme_id, cx);
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(std::slice::from_ref(&account))
        {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar import failed: {err:#}");
                google_oauth_log(format!("import_calendar.disabled: {err:#}"));
                self.show_google_calendar_error("Google Calendar import", format!("{err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        let sources = active_google_calendar_sources(&self.workspace);
        google_oauth_log(format!(
            "import_calendar.requested account={} calendar_id={}",
            google_account_label(&account),
            calendar_id
        ));

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    google_oauth_log("import_calendar.worker start");
                    let result = run_google_calendar_import_existing_account_calendar(
                        config,
                        account,
                        sources,
                        calendar_id,
                    )
                    .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(parent, result, None, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_import(
                                    parent,
                                    Err("Google Calendar import worker stopped".to_string()),
                                    None,
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_scheme_refresh(
        &mut self,
        scheme_id: knotq_model::SchemeId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress) {
            return;
        }

        let Some(source) = self.workspace.scheme(scheme_id).and_then(|scheme| {
            let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                return None;
            };
            (source.provider == CalendarProvider::Google).then_some(source.clone())
        }) else {
            return;
        };

        let accounts = self
            .settings
            .google_accounts
            .iter()
            .filter(|account| google_account_matches_calendar_source(account, &source))
            .cloned()
            .collect::<Vec<_>>();
        if accounts.is_empty() {
            eprintln!("Google Calendar refresh failed: no stored Google account for this calendar");
            google_oauth_log("refresh.failed no stored Google account for this calendar");
            self.google_oauth_status = GoogleOAuthStatus::Error;
            cx.notify();
            return;
        }

        let config = match google_oauth_config_for_existing_accounts(&accounts) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar refresh failed: {err:#}");
                google_oauth_log(format!("refresh.disabled: {err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        google_oauth_log(format!(
            "refresh.requested account={} calendar_id={}",
            google_calendar_source_target_label(&source),
            source.calendar_id
        ));
        let sources = vec![ExistingGoogleCalendarSource {
            account_id: source.account_id,
            account_email: source.account_email,
            calendar_id: source.calendar_id,
            sync_token: source.sync_token,
        }];

        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    google_oauth_log("refresh.worker start");
                    let result = run_google_calendar_background_sync(config, accounts, sources)
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(result, None, cx);
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(
                                    Err("Google Calendar refresh worker stopped".to_string()),
                                    None,
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    pub(crate) fn start_google_calendar_scheme_reconnect(
        &mut self,
        scheme_id: knotq_model::SchemeId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress)
            && self.google_oauth_cancel_token.is_none()
        {
            return;
        }

        let Some(source) = self.workspace.scheme(scheme_id).and_then(|scheme| {
            let SchemeSource::ImportedCalendar(source) = &scheme.source else {
                return None;
            };
            (source.provider == CalendarProvider::Google).then_some(source.clone())
        }) else {
            return;
        };

        let config = match google_oauth_config_from_build() {
            Ok(config) => config,
            Err(err) => {
                eprintln!("Google Calendar reconnect failed: {err:#}");
                google_oauth_log(format!("reconnect.disabled: {err:#}"));
                self.show_google_calendar_error("Google Calendar reconnect", format!("{err:#}"));
                self.google_oauth_status = GoogleOAuthStatus::Error;
                cx.notify();
                return;
            }
        };
        google_oauth_log(format!(
            "reconnect.requested account={} calendar_id={}",
            google_calendar_source_target_label(&source),
            source.calendar_id
        ));

        let cancel_token = self.begin_google_oauth_browser_flow();
        self.google_oauth_status = GoogleOAuthStatus::InProgress;
        let task = cx.spawn(
            async move |weak: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (tx, rx) = mpsc::channel();
                let worker_cancel_token = cancel_token.clone();
                let finish_cancel_token = cancel_token.clone();
                std::thread::spawn(move || {
                    google_oauth_log("reconnect.worker start");
                    let result =
                        run_google_calendar_scheme_reconnect(config, source, worker_cancel_token)
                            .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });

                loop {
                    match rx.try_recv() {
                        Ok(result) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(
                                    result,
                                    Some(finish_cancel_token.clone()),
                                    cx,
                                );
                            });
                            break;
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            cx.background_executor()
                                .timer(StdDuration::from_millis(100))
                                .await;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            let _ = weak.update(cx, |app, cx| {
                                app.finish_google_calendar_scheme_refresh(
                                    Err("Google Calendar reconnect worker stopped".to_string()),
                                    Some(finish_cancel_token.clone()),
                                    cx,
                                );
                            });
                            break;
                        }
                    }
                }
            },
        );
        self.google_oauth_task = Some(task);
        cx.notify();
    }

    fn google_calendar_background_snapshot(&self) -> Option<GoogleCalendarBackgroundSnapshot> {
        if self.google_oauth_task.is_some()
            || self.settings.google_accounts.is_empty()
            || matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress)
        {
            return None;
        }
        let sources = google_calendar_sources(&self.workspace);
        if sources.is_empty() {
            return None;
        }
        let accounts = self
            .settings
            .google_accounts
            .iter()
            .filter(|account| google_account_has_local_credentials(account))
            .filter(|account| {
                sources
                    .iter()
                    .any(|source| existing_source_matches_google_account(source, account))
            })
            .cloned()
            .collect::<Vec<_>>();
        if accounts.is_empty() {
            return None;
        }
        let config = google_oauth_config_for_existing_accounts(&accounts)
            .map_err(|err| {
                eprintln!("background Google Calendar sync disabled: {err:#}");
                google_oauth_log(format!("background_sync.disabled: {err:#}"));
                err
            })
            .ok()?;
        Some(GoogleCalendarBackgroundSnapshot {
            config,
            accounts,
            sources,
        })
    }

    fn finish_google_calendar_import(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cancel_token: Option<Arc<AtomicBool>>,
        cx: &mut Context<Self>,
    ) {
        if !self.finish_google_oauth_task(cancel_token.as_ref()) {
            return;
        }
        self.finish_google_sync_result(result, parent, true, true, true, "import", cx);
    }

    fn finish_google_calendar_picker_load(
        &mut self,
        parent: FolderId,
        result: std::result::Result<GoogleCalendarPickerLoadResult, String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .google_calendar_picker
            .as_ref()
            .is_some_and(|picker| picker.parent == parent)
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

    fn finish_google_calendar_scheme_refresh(
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
            self.workspace.root,
            false,
            false,
            true,
            "refresh",
            cx,
        );
    }

    fn finish_google_calendar_background_sync(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        cx: &mut Context<Self>,
    ) {
        self.finish_google_sync_result(
            result,
            self.workspace.root,
            false,
            false,
            false,
            "background sync",
            cx,
        );
    }

    fn finish_google_sync_result(
        &mut self,
        result: std::result::Result<GoogleCalendarImportResult, String>,
        parent: FolderId,
        create_missing: bool,
        open_first_imported: bool,
        always_notify: bool,
        label: &str,
        cx: &mut Context<Self>,
    ) {
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
                            format!("Google Calendar {label} failed"),
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
                            format!("Google Calendar {label} failed"),
                            err.clone(),
                        );
                        self.google_oauth_status = GoogleOAuthStatus::Error;
                    }
                    cx.notify();
                }
            }
        }
    }

    fn show_google_calendar_error(&mut self, title: impl Into<String>, message: impl Into<String>) {
        self.notice_modal = Some(NoticeModal {
            title: title.into(),
            message: message.into(),
            button_label: "OK".to_string(),
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

fn run_google_calendar_picker_load(
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

fn run_google_calendar_import(
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

fn run_google_calendar_import_existing_account_calendar(
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

fn run_google_calendar_scheme_reconnect(
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

fn run_google_calendar_background_sync(
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
