    use super::*;

    fn account() -> GoogleOAuthAccount {
        GoogleOAuthAccount {
            account_id: "acct".to_string(),
            email: Some("user@example.com".to_string()),
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            expires_at: None,
            scope: "https://www.googleapis.com/auth/calendar.events.readonly".to_string(),
        }
    }

    fn google_config() -> GoogleOAuthConfig {
        GoogleOAuthConfig {
            client_id: "desktop-client".to_string(),
            client_secret: "desktop-secret".to_string(),
        }
    }

    fn dt(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn external(event_id: &str) -> ExternalItemSource {
        ExternalItemSource {
            provider: CalendarProvider::Google,
            account_id: "acct".to_string(),
            calendar_id: "cal".to_string(),
            event_id: event_id.to_string(),
            instance_id: None,
            updated_at: None,
        }
    }

    fn imported_calendar() -> ImportedGoogleCalendar {
        ImportedGoogleCalendar {
            account_id: "acct".to_string(),
            account_email: Some("user@example.com".to_string()),
            calendar_id: "cal".to_string(),
            name: "Calendar".to_string(),
            color_index: 0,
            sync_token: None,
            full_sync: true,
            items: Vec::new(),
            deleted: Vec::new(),
        }
    }

    fn imported_google_scheme(name: &str) -> Scheme {
        let mut scheme = Scheme::new(name, 0);
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "acct".to_string(),
            account_email: Some("user@example.com".to_string()),
            calendar_id: "cal".to_string(),
            sync_token: None,
            read_only: true,
            last_synced_at: None,
        });
        scheme
    }

    #[test]
    fn google_oauth_client_id_comes_from_compile_time_env() {
        assert_eq!(
            google_oauth_client_id_from_compiled(Some(
                " 419826075228-07g85tu69ug0hvkepfdi6qv4p12ulolv.apps.googleusercontent.com "
            ))
            .as_deref(),
            Some("419826075228-07g85tu69ug0hvkepfdi6qv4p12ulolv.apps.googleusercontent.com")
        );
    }

    #[test]
    fn google_oauth_client_id_missing_compile_time_env_is_config_error() {
        assert_eq!(google_oauth_client_id_from_compiled(None), None);
    }

    #[test]
    fn google_oauth_client_secret_comes_from_compile_time_env() {
        assert_eq!(
            google_oauth_client_secret_from_compiled(Some(" desktop-secret ")).as_deref(),
            Some("desktop-secret")
        );
    }

    #[test]
    fn google_oauth_client_secret_missing_compile_time_env_is_config_error() {
        assert_eq!(google_oauth_client_secret_from_compiled(None), None);
    }

    #[test]
    fn google_oauth_browser_cancel_and_timeout_errors_are_non_modal() {
        assert!(is_google_oauth_browser_cancel_or_timeout(
            GOOGLE_OAUTH_CALLBACK_CANCELLED
        ));
        assert!(is_google_oauth_browser_cancel_or_timeout(
            GOOGLE_OAUTH_CALLBACK_TIMEOUT
        ));
        assert!(is_google_oauth_browser_cancel_or_timeout(
            GOOGLE_OAUTH_ACCESS_DENIED
        ));
        assert!(!is_google_oauth_browser_cancel_or_timeout(
            "Google OAuth HTTP 400: invalid_request"
        ));
    }

    #[test]
    fn wait_for_oauth_code_returns_when_cancelled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let cancel_token = AtomicBool::new(true);

        let err = wait_for_oauth_code(
            &listener,
            "state",
            StdDuration::from_secs(60),
            "ok",
            "failed",
            Some(&cancel_token),
        )
        .unwrap_err();

        assert!(format!("{err:#}").contains(GOOGLE_OAUTH_CALLBACK_CANCELLED));
    }

    #[test]
    fn google_auth_url_uses_s256_pkce_challenge() {
        let url = google_auth_url(
            "desktop-client",
            "http://127.0.0.1:12345",
            "openid email",
            "state",
            "challenge",
        );
        let params = query_params(&url).unwrap();

        assert_eq!(
            params.get("client_id").map(String::as_str),
            Some("desktop-client")
        );
        assert_eq!(
            params.get("code_challenge").map(String::as_str),
            Some("challenge")
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            params.get("access_type").map(String::as_str),
            Some("offline")
        );
        assert!(!params.contains_key("client_secret"));
    }

    #[test]
    fn google_refresh_client_id_uses_configured_desktop_client_id() {
        let mut config = google_config();
        config.client_id = "compiled-client".to_string();
        let mut account = account();
        account.client_id = "stored-client".to_string();

        assert_eq!(
            google_oauth_client_id_for_refresh(&config, &account),
            "compiled-client"
        );
    }

    #[test]
    fn google_refresh_client_id_uses_configured_desktop_client_for_legacy_accounts() {
        let mut config = google_config();
        config.client_id = "compiled-client".to_string();
        let mut account = account();
        account.client_id.clear();

        assert_eq!(
            google_oauth_client_id_for_refresh(&config, &account),
            "compiled-client"
        );
    }

    #[test]
    fn google_auth_code_exchange_form_uses_pkce_and_desktop_client_id() {
        let config = google_config();
        let form = google_auth_code_exchange_form(
            &config,
            "http://127.0.0.1:12345",
            "auth-code",
            "pkce-verifier",
        )
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            form.get("client_id").map(String::as_str),
            Some("desktop-client")
        );
        assert_eq!(
            form.get("client_secret").map(String::as_str),
            Some("desktop-secret")
        );
        assert_eq!(
            form.get("code_verifier").map(String::as_str),
            Some("pkce-verifier")
        );
        assert_eq!(form.get("code").map(String::as_str), Some("auth-code"));
        assert_eq!(
            form.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:12345")
        );
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
    }

    #[test]
    fn google_refresh_form_uses_desktop_client_id() {
        let config = google_config();
        let form = google_refresh_token_form(&config, "refresh-token")
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            form.get("client_id").map(String::as_str),
            Some("desktop-client")
        );
        assert_eq!(
            form.get("client_secret").map(String::as_str),
            Some("desktop-secret")
        );
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("refresh_token")
        );
        assert_eq!(
            form.get("refresh_token").map(String::as_str),
            Some("refresh-token")
        );
    }

    #[test]
    fn google_invalid_grant_refresh_error_is_terminal() {
        let err = anyhow!("invalid_grant");

        assert!(is_terminal_google_refresh_error(&err));
    }

    #[test]
    fn google_client_secret_missing_http_error_explains_compile_time_secret_requirement() {
        let message = format_google_http_error(
            400,
            r#"{"error":"invalid_request","error_description":"client_secret is missing."}"#,
        );

        assert!(message.contains("client_secret was missing"));
        assert!(message.contains(GOOGLE_OAUTH_CLIENT_SECRET_ENV));
        assert!(message.contains("compile time"));
    }

    #[test]
    fn google_event_to_item_preserves_calendar_identity_and_rrule() {
        let event = GoogleEvent {
            id: "event-1".to_string(),
            status: Some("confirmed".to_string()),
            summary: Some("Standup".to_string()),
            start: Some(GoogleEventDateTime {
                date: None,
                date_time: Some(dt("2026-05-18T13:00:00Z")),
            }),
            end: Some(GoogleEventDateTime {
                date: None,
                date_time: Some(dt("2026-05-18T13:30:00Z")),
            }),
            updated: Some(dt("2026-05-18T12:00:00Z")),
            recurrence: Some(vec!["RRULE:FREQ=WEEKLY;BYDAY=MO".to_string()]),
            recurring_event_id: None,
        };

        let item = google_event_to_item(&account(), "cal", &event).unwrap();

        assert_eq!(item.text(), "Standup");
        assert_eq!(item.marker, ItemMarker::Checkbox);
        assert_eq!(item.start, Some(dt("2026-05-18T13:00:00Z")));
        assert_eq!(item.end, Some(dt("2026-05-18T13:30:00Z")));
        let external = item.external.unwrap();
        assert_eq!(external.provider, CalendarProvider::Google);
        assert_eq!(external.account_id, "acct");
        assert_eq!(external.calendar_id, "cal");
        assert_eq!(external.event_id, "event-1");
        assert_eq!(external.updated_at, Some(dt("2026-05-18T12:00:00Z")));
        let repeats = item.repeats.unwrap();
        assert_eq!(repeats.rrules, vec!["FREQ=WEEKLY;BYDAY=MO"]);
    }

    #[test]
    fn google_calendar_metadata_preserves_existing_local_color() {
        let mut scheme = Scheme::new("Local name", 4);
        let changed = apply_google_calendar_metadata(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Google name".to_string(),
                color_index: 1,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: Vec::new(),
                deleted: Vec::new(),
            },
            false,
        );

        assert!(changed);
        assert_eq!(scheme.name, "Local name");
        assert_eq!(scheme.color_index, 4);
        let SchemeSource::ImportedCalendar(source) = &scheme.source else {
            panic!("expected imported calendar source");
        };
        assert_eq!(source.provider, CalendarProvider::Google);
        assert_eq!(source.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(source.sync_token.as_deref(), Some("next"));
    }

    #[test]
    fn google_calendar_selected_existing_calendar_matches_locally_without_import_worker() {
        let account = account();
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Existing calendar", 2);
        let scheme_id = scheme.id;
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "user@example.com".to_string(),
            account_email: None,
            calendar_id: "cal".to_string(),
            sync_token: Some("token".to_string()),
            read_only: true,
            last_synced_at: None,
        });
        workspace.schemes.insert(scheme_id, scheme);

        assert_eq!(
            find_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            Some(scheme_id)
        );
    }

    #[test]
    fn google_calendar_archived_calendar_does_not_count_as_added() {
        let account = account();
        let mut workspace = Workspace::new();
        let mut scheme = Scheme::new("Archived calendar", 2);
        let scheme_id = scheme.id;
        scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "user@example.com".to_string(),
            account_email: None,
            calendar_id: "cal".to_string(),
            sync_token: Some("token".to_string()),
            read_only: true,
            last_synced_at: None,
        });
        workspace.schemes.insert(scheme_id, scheme);
        workspace.mark_scheme_deleted(scheme_id);

        assert!(active_google_calendar_sources(&workspace).is_empty());
        assert_eq!(
            find_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            None
        );
        assert_eq!(
            find_archived_google_calendar_scheme_for_account(&workspace, &account, "cal"),
            Some(scheme_id)
        );
    }

    #[test]
    fn active_google_calendar_scheme_ids_use_sidebar_order_and_skip_archived() {
        let mut workspace = Workspace::new();
        let root = workspace.root;
        let first = imported_google_scheme("First");
        let first_id = first.id;
        let second = imported_google_scheme("Second");
        let second_id = second.id;
        let archived = imported_google_scheme("Archived");
        let archived_id = archived.id;
        workspace.schemes.insert(first_id, first);
        workspace.schemes.insert(second_id, second);
        workspace.schemes.insert(archived_id, archived);
        workspace.folders.get_mut(&root).unwrap().children.extend([
            NodeRef::Scheme(second_id),
            NodeRef::Scheme(first_id),
            NodeRef::Scheme(archived_id),
        ]);
        workspace.mark_scheme_deleted(archived_id);

        assert_eq!(
            active_google_calendar_scheme_ids(&workspace, &imported_calendar()),
            vec![second_id, first_id]
        );
    }

    #[test]
    fn incremental_sync_updates_and_removes_matching_external_items() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Old");
        existing.external = Some(external("stay"));
        let existing_id = existing.id;
        let mut removed = Item::new("Remove");
        removed.external = Some(external("gone"));
        scheme.items = vec![existing, removed];

        let mut updated = Item::new("Updated");
        updated.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![updated],
                deleted: vec![GoogleExternalEventKey {
                    event_id: "gone".to_string(),
                    instance_id: None,
                }],
            },
        );

        assert!(changed);
        assert_eq!(scheme.items.len(), 1);
        assert_eq!(scheme.items[0].id, existing_id);
        assert_eq!(scheme.items[0].text(), "Updated");
        assert_eq!(scheme.items[0].external.as_ref().unwrap().event_id, "stay");
    }

    #[test]
    fn incremental_sync_without_item_changes_is_stable() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same");
        existing.external = Some(external("stay"));
        let existing_id = existing.id;
        scheme.items = vec![existing];

        let mut imported = Item::new("Same");
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert_eq!(scheme.items.len(), 1);
        assert_eq!(scheme.items[0].id, existing_id);
        assert_eq!(scheme.items[0].text(), "Same");
    }

    #[test]
    fn incremental_sync_preserves_local_completion_state_for_same_event_time() {
        let start = dt("2026-05-18T13:00:00Z");
        let end = dt("2026-05-18T13:30:00Z");
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same").with_start(start).with_end(end).done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same").with_start(start).with_end(end);
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert!(scheme.items[0].single_state().is_done());
    }

    #[test]
    fn full_sync_preserves_local_completion_state_for_same_event_time() {
        let start = dt("2026-05-18T13:00:00Z");
        let end = dt("2026-05-18T13:30:00Z");
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same").with_start(start).with_end(end).done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same").with_start(start).with_end(end);
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: true,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(!changed);
        assert!(scheme.items[0].single_state().is_done());
    }

    #[test]
    fn sync_resets_completion_state_when_event_time_changes() {
        let mut scheme = Scheme::new("Work", 0);
        let mut existing = Item::new("Same")
            .with_start(dt("2026-05-18T13:00:00Z"))
            .with_end(dt("2026-05-18T13:30:00Z"))
            .done();
        existing.external = Some(external("stay"));
        scheme.items = vec![existing];

        let mut imported = Item::new("Same")
            .with_start(dt("2026-05-19T13:00:00Z"))
            .with_end(dt("2026-05-19T13:30:00Z"));
        imported.external = Some(external("stay"));

        let changed = apply_google_calendar_items(
            &mut scheme,
            &ImportedGoogleCalendar {
                account_id: "acct".to_string(),
                account_email: Some("user@example.com".to_string()),
                calendar_id: "cal".to_string(),
                name: "Work".to_string(),
                color_index: 0,
                sync_token: Some("next".to_string()),
                full_sync: false,
                items: vec![imported],
                deleted: Vec::new(),
            },
        );

        assert!(changed);
        assert!(!scheme.items[0].single_state().is_done());
    }
