use super::super::*;

use super::workers::{
    run_google_calendar_background_sync, run_google_calendar_import,
    run_google_calendar_import_existing_account_calendar, run_google_calendar_picker_load,
    run_google_calendar_scheme_reconnect,
};

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

    pub(super) fn finish_google_oauth_task(&mut self, cancel_token: Option<&Arc<AtomicBool>>) -> bool {
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
                                    Err(knotq_l10n::t(
                                        "google.calendar.error.selector_worker_stopped",
                                    )
                                    .to_string()),
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
                self.show_google_calendar_error(
                    knotq_l10n::t("google.calendar.import_title"),
                    format!("{err:#}"),
                );
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
                                    Err(knotq_l10n::t("google.oauth.error.worker_stopped")
                                        .to_string()),
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
                knotq_l10n::t("google.calendar.import_title"),
                knotq_l10n::t("google.calendar.account_not_connected"),
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
                self.show_google_calendar_error(
                    knotq_l10n::t("google.calendar.import_title"),
                    format!("{err:#}"),
                );
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
                                    Err(knotq_l10n::t(
                                        "google.calendar.error.import_worker_stopped",
                                    )
                                    .to_string()),
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
                                    Err(knotq_l10n::t(
                                        "google.calendar.error.refresh_worker_stopped",
                                    )
                                    .to_string()),
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
                self.show_google_calendar_error(
                    knotq_l10n::t("google.calendar.reconnect_title"),
                    format!("{err:#}"),
                );
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
                                    Err(knotq_l10n::t(
                                        "google.calendar.error.reconnect_worker_stopped",
                                    )
                                    .to_string()),
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
}
