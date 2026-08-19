use super::*;

pub(crate) fn run_google_oauth(
    config: GoogleOAuthConfig,
    cancel_token: &AtomicBool,
) -> Result<GoogleOAuthAccount> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind OAuth loopback listener")?;
    listener
        .set_nonblocking(true)
        .context("make OAuth loopback listener nonblocking")?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let state = random_token(32);
    let code_verifier = random_token(96);
    let code_challenge = code_challenge(&code_verifier);
    let scope = GOOGLE_OAUTH_SCOPES.join(" ");
    let auth_url = google_auth_url(
        &config.client_id,
        &redirect_uri,
        &scope,
        &state,
        &code_challenge,
    );

    google_oauth_log(format!("oauth.browser_open scopes=\"{scope}\""));
    open_browser(&auth_url)?;
    // The exchange runs while the browser is still waiting on our response, so
    // the tab can be told the truth: a grant that ticked no calendar box has to
    // read as "not connected" there, not only back in the app.
    let token = match wait_for_oauth_callback(
        &listener,
        &state,
        StdDuration::from_secs(120),
        |err| {
            if err.downcast_ref::<GoogleCalendarScopeDenied>().is_some() {
                knotq_l10n::t("google.oauth.callback.scope_denied_body").to_string()
            } else {
                knotq_l10n::t("google.oauth.callback.failure_body").to_string()
            }
        },
        Some(cancel_token),
        |code| {
            google_oauth_log("oauth.callback ok");
            google_oauth_log("oauth.exchange start");
            let token = match exchange_auth_code(&config, &redirect_uri, code, &code_verifier) {
                Ok(token) => {
                    google_oauth_log("oauth.exchange ok");
                    token
                }
                Err(err) => {
                    google_oauth_log(format!("oauth.exchange failed: {err:#}"));
                    return Err(err);
                }
            };
            // Google reports what the user actually ticked, which can be less
            // than what was asked for: the calendar checkboxes are not ticked for
            // them, so finishing the screen without touching anything grants
            // nothing but `openid`/`email`.
            let granted_scope = token
                .scope
                .clone()
                .unwrap_or_else(|| GOOGLE_OAUTH_SCOPES.join(" "));
            let missing_scopes = missing_google_calendar_scopes(&granted_scope);
            if !missing_scopes.is_empty() {
                google_oauth_log(format!(
                    "oauth.scope_denied granted=\"{granted_scope}\" missing=\"{}\"",
                    missing_scopes.join(" ")
                ));
                return Err(anyhow!(GoogleCalendarScopeDenied));
            }
            Ok((
                token,
                knotq_l10n::t("google.oauth.callback.success_body").to_string(),
            ))
        },
    ) {
        Ok(token) => token,
        Err(err) => {
            google_oauth_log(format!("oauth.callback failed: {err:#}"));
            return Err(err);
        }
    };
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!(knotq_l10n::t("google.oauth.error.no_refresh_token")))?;
    let granted_scope = token
        .scope
        .clone()
        .unwrap_or_else(|| GOOGLE_OAUTH_SCOPES.join(" "));
    let claims = token.id_token.as_deref().and_then(decode_id_token_claims);
    let account_id = claims
        .as_ref()
        .and_then(|claims| claims.sub.clone())
        .or_else(|| claims.as_ref().and_then(|claims| claims.email.clone()))
        .unwrap_or_else(|| "google".to_string());
    let expires_at = token
        .expires_in
        .map(|seconds| Utc::now() + Duration::seconds(seconds));

    let account = GoogleOAuthAccount {
        account_id,
        email: claims.and_then(|claims| claims.email),
        client_id: config.client_id,
        access_token: token.access_token,
        refresh_token,
        expires_at,
        scope: granted_scope,
        // Desktop runs the loopback OAuth flow itself and holds a refresh
        // token, so the core renews access tokens without the shell's help.
        token_source: knotq_model::GoogleTokenSource::OAuthRefreshToken,
        needs_reauth: false,
    };
    google_oauth_log(format!(
        "oauth.account connected account={} scope=\"{}\"",
        google_account_label(&account),
        account.scope
    ));
    Ok(account)
}

pub(crate) fn google_auth_url(
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
) -> String {
    let params = [
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("state", state),
        ("code_challenge", code_challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{GOOGLE_AUTH_URL}?{query}")
}

/// The user finished Google's consent screen without granting the calendar
/// reads. Typed rather than a message, so the browser page and the in-app
/// notice can each be chosen from it without matching on localized text.
#[derive(Debug)]
pub(crate) struct GoogleCalendarScopeDenied;

impl std::fmt::Display for GoogleCalendarScopeDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(knotq_l10n::t("google.oauth.error.calendar_scope_denied"))
    }
}

impl std::error::Error for GoogleCalendarScopeDenied {}

/// Block on a loopback OAuth/PKCE redirect, returning the `code` query parameter
/// once the browser hits the listener (and the `state` matches). Shared by the
/// Google Calendar import and the sync browser sign-in; callers pass the success
/// and failure pages shown in the browser tab.
pub(crate) fn wait_for_oauth_code(
    listener: &TcpListener,
    expected_state: &str,
    timeout: StdDuration,
    success_body: &str,
    failure_body: &str,
    cancel_token: Option<&AtomicBool>,
) -> Result<String> {
    wait_for_oauth_callback(
        listener,
        expected_state,
        timeout,
        |_| failure_body.to_string(),
        cancel_token,
        |code| Ok((code.to_string(), success_body.to_string())),
    )
}

/// The same loopback wait, with the browser's page decided by `finish` rather
/// than by whether a code arrived.
///
/// The tab is still open and waiting on our response when `finish` runs, so
/// whatever the code turns out to be worth — a grant that covers nothing, an
/// exchange Google rejects — can be said on the page the user is already
/// looking at. Reporting only inside the app is how a user ends up with a
/// browser that said "connected" and an app that says permission denied.
pub(crate) fn wait_for_oauth_callback<T>(
    listener: &TcpListener,
    expected_state: &str,
    timeout: StdDuration,
    failure_body: impl Fn(&anyhow::Error) -> String,
    cancel_token: Option<&AtomicBool>,
    finish: impl FnOnce(&str) -> Result<(T, String)>,
) -> Result<T> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if cancel_token.is_some_and(|cancel_token| cancel_token.load(Ordering::SeqCst)) {
            bail!(google_oauth_error_cancelled());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                // The listener is non-blocking so this wait stays cancellable,
                // and on macOS the accepted socket inherits that flag: reading
                // it before the browser's bytes land fails with EWOULDBLOCK
                // ("Resource temporarily unavailable", os error 35) and the
                // whole connect dies on a lost race. Put this socket back to
                // blocking, with a timeout so a silent client cannot wedge us.
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(CALLBACK_READ_TIMEOUT));
                let outcome = read_oauth_callback(&mut stream, expected_state)
                    .and_then(|code| finish(&code));
                let body = match &outcome {
                    Ok((_, body)) => body.clone(),
                    Err(err) => failure_body(err),
                };
                let _ = write_http_response(&mut stream, &body);
                return outcome.map(|(value, _)| value);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(StdDuration::from_millis(100));
            }
            Err(err) => return Err(err).context("accept OAuth callback"),
        }
    }
    bail!(google_oauth_error_timeout())
}

pub(crate) fn read_oauth_callback(stream: &mut TcpStream, expected_state: &str) -> Result<String> {
    // Everything needed is on the request line, but one read is not guaranteed
    // to hold it: keep reading until that line ends, or the peer stops talking.
    let mut buffer = [0u8; 4096];
    let mut filled = 0usize;
    let request = loop {
        let len = stream
            .read(&mut buffer[filled..])
            .context("read OAuth callback")?;
        filled += len;
        let request = String::from_utf8_lossy(&buffer[..filled]).into_owned();
        if len == 0 || request.contains('\n') || filled == buffer.len() {
            break request;
        }
    };
    let request_target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid OAuth callback request"))?;
    let params = query_params(request_target)?;
    if params.get("state").map(String::as_str) != Some(expected_state) {
        bail!(knotq_l10n::t("google.oauth.error.unexpected_state"));
    }
    if let Some(error) = params.get("error") {
        bail!(knotq_l10n::t_with(
            "google.oauth.error.provider_error",
            &[("error", error)]
        ));
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow!(knotq_l10n::t("google.oauth.error.missing_code")))
}

pub(crate) fn query_params(request_target: &str) -> Result<HashMap<String, String>> {
    let query = request_target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or("");
    let query = query.split('#').next().unwrap_or(query);
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = urlencoding::decode(key)?.into_owned();
        let value = urlencoding::decode(value)?.into_owned();
        params.insert(key, value);
    }
    Ok(params)
}

pub(crate) fn exchange_auth_code(
    config: &GoogleOAuthConfig,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Result<GoogleTokenResponse> {
    let form = google_auth_code_exchange_form(config, redirect_uri, code, code_verifier);
    post_token_form(&form, "parse Google OAuth token response")
}

/// POST a form to the Google token endpoint and decode the token response.
/// Shared by the authorization-code exchange and refresh-token flows.
fn post_token_form(
    form: &[(&'static str, String)],
    parse_context: &'static str,
) -> Result<GoogleTokenResponse> {
    let form_refs = form
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();

    ureq::post(GOOGLE_TOKEN_URL)
        .send_form(&form_refs)
        .map_err(google_http_error)?
        .into_json::<GoogleTokenResponse>()
        .context(parse_context)
}

pub(crate) fn google_auth_code_exchange_form(
    config: &GoogleOAuthConfig,
    redirect_uri: &str,
    code: &str,
    code_verifier: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", config.client_id.clone()),
        google_desktop_client_secret_form_field(config),
        ("code", code.to_string()),
        ("code_verifier", code_verifier.to_string()),
        ("grant_type", "authorization_code".to_string()),
        ("redirect_uri", redirect_uri.to_string()),
    ]
}

pub(crate) fn refresh_google_access_token_if_needed(
    config: &GoogleOAuthConfig,
    account: &mut GoogleOAuthAccount,
) -> Result<()> {
    let still_valid = account
        .expires_at
        .is_some_and(|expires_at| expires_at > Utc::now() + Duration::seconds(60));
    if still_valid {
        return Ok(());
    }
    let label = google_account_label(account).to_string();
    google_oauth_log(format!(
        "token.refresh start account={label} stored_scope=\"{}\"",
        account.scope
    ));

    let client_id = google_oauth_client_id_for_refresh(config, account);
    let token = match request_google_refresh_token(config, &account.refresh_token) {
        Ok(token) => token,
        Err(err) => return fail_google_refresh(err, account, &label),
    };

    account.access_token = token.access_token;
    if account.client_id.trim() != client_id {
        account.client_id = client_id;
    }
    account.expires_at = token
        .expires_in
        .map(|seconds| Utc::now() + Duration::seconds(seconds));
    if let Some(scope) = token.scope {
        account.scope = scope;
    }
    google_oauth_log(format!(
        "token.refresh ok account={label} scope=\"{}\"",
        account.scope
    ));
    // A grant can be narrowed after the fact from the user's Google account
    // page, and the refresh response is where that first shows up. Reporting it
    // here beats letting the next Calendar call fail with a raw 403 body.
    let missing_scopes = missing_google_calendar_scopes(&account.scope);
    if !missing_scopes.is_empty() {
        google_oauth_log(format!(
            "token.refresh scope_insufficient account={label} missing=\"{}\"",
            missing_scopes.join(" ")
        ));
        account.needs_reauth = true;
        bail!(knotq_l10n::t_with(
            "google.calendar.error.permission_denied",
            &[("account", label.as_str())]
        ));
    }
    account.needs_reauth = false;
    Ok(())
}

/// Whether a Calendar API failure means this account's Google grant no longer
/// covers what KnotQ needs: a permission the user never ticked, or an
/// authorization they have since revoked. Reconnecting fixes those; retrying
/// never does.
pub(crate) fn is_google_authorization_error(err: &GoogleApiError) -> bool {
    match err.status {
        Some(401) => true,
        Some(403) => {
            err.message.contains("ACCESS_TOKEN_SCOPE_INSUFFICIENT")
                || err.message.contains("insufficientPermissions")
        }
        _ => false,
    }
}

/// Turns a Calendar API failure into the message the user sees, flagging the
/// account when the failure is one only a reconnect can clear. Without this the
/// UI shows Google's raw JSON error body, which says nothing a user can act on.
pub(crate) fn google_calendar_request_error(
    account: &mut GoogleOAuthAccount,
    err: GoogleApiError,
) -> anyhow::Error {
    if !is_google_authorization_error(&err) {
        return anyhow!(err);
    }
    account.needs_reauth = true;
    google_oauth_log(format!(
        "account.needs_reauth account={} status={:?}: {}",
        google_account_label(account),
        err.status,
        err.message
    ));
    anyhow!(knotq_l10n::t_with(
        "google.calendar.error.permission_denied",
        &[("account", google_account_label(account))]
    ))
}

pub(crate) fn request_google_refresh_token(
    config: &GoogleOAuthConfig,
    refresh_token: &str,
) -> Result<GoogleTokenResponse> {
    let form = google_refresh_token_form(config, refresh_token);
    post_token_form(&form, "parse Google OAuth refresh response")
}

pub(crate) fn google_refresh_token_form(
    config: &GoogleOAuthConfig,
    refresh_token: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("client_id", config.client_id.clone()),
        google_desktop_client_secret_form_field(config),
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
    ]
}

pub(crate) fn google_desktop_client_secret_form_field(
    config: &GoogleOAuthConfig,
) -> (&'static str, String) {
    // Google Desktop OAuth clients can require client_secret at the token endpoint even
    // with PKCE. In a shipped desktop app this is not confidential; it is the
    // installed-app credential Google expects us to send.
    // https://discuss.google.dev/t/is-it-ok-to-put-a-client-secret-in-a-desktop-app/296820/6
    // https://developers.google.com/identity/protocols/oauth2/native-app
    ("client_secret", config.client_secret.clone())
}

pub(crate) fn fail_google_refresh(
    err: anyhow::Error,
    account: &mut GoogleOAuthAccount,
    label: &str,
) -> Result<()> {
    google_oauth_log(format!("token.refresh failed account={label}: {err:#}"));
    if is_terminal_google_refresh_error(&err) {
        account.access_token.clear();
        account.refresh_token.clear();
        account.expires_at = None;
        google_oauth_log(format!(
            "token.refresh cleared_local_credentials account={label}"
        ));
    }
    Err(err)
}

pub(crate) fn google_oauth_client_id_for_refresh(
    config: &GoogleOAuthConfig,
    _account: &GoogleOAuthAccount,
) -> String {
    config.client_id.trim().to_string()
}

pub(crate) fn import_google_account_calendars(
    account: &mut GoogleOAuthAccount,
    existing_sources: &[ExistingGoogleCalendarSource],
    mode: GoogleCalendarImportMode,
    target_calendar_id: Option<&str>,
) -> Result<(Vec<ImportedGoogleCalendar>, Vec<String>)> {
    let calendars = match list_google_calendars(&account.access_token) {
        Ok(calendars) => {
            google_oauth_log(format!(
                "calendar_list ok account={} calendars={}",
                google_account_label(account),
                calendars.len()
            ));
            account.needs_reauth = false;
            calendars
        }
        Err(err) => {
            google_oauth_log(format!(
                "calendar_list failed account={}: {err}",
                google_account_label(account)
            ));
            return Err(google_calendar_request_error(account, err));
        }
    };
    let fallback_count = calendars.len().max(1);
    let mut imported = Vec::new();
    let mut failures = Vec::new();

    for (index, calendar) in calendars.into_iter().enumerate() {
        if target_calendar_id.is_some_and(|target| target != calendar.id) {
            continue;
        }
        let existing = existing_sources.iter().find(|source| {
            source.calendar_id == calendar.id
                && existing_source_matches_google_account(source, account)
        });
        match mode {
            GoogleCalendarImportMode::ExistingOnly if existing.is_none() => continue,
            GoogleCalendarImportMode::MissingOnly if existing.is_some() => continue,
            _ => {}
        }
        let sync_token = existing.and_then(|source| source.sync_token.clone());
        // Two statements, not one: the request borrows the account's token, and
        // the error mapping needs the account mutably to flag a lost grant.
        let events = list_google_events(&account.access_token, &calendar.id, sync_token);
        let events = match events.map_err(|err| google_calendar_request_error(account, err)) {
            Ok(events) => {
                google_oauth_log(format!(
                    "events.list ok account={} calendar={} events={} full_sync={}",
                    google_account_label(account),
                    google_calendar_name(&calendar),
                    events.events.len(),
                    events.full_sync
                ));
                events
            }
            Err(err) => {
                google_oauth_log(format!(
                    "events.list failed account={} calendar={}: {err}",
                    google_account_label(account),
                    google_calendar_name(&calendar)
                ));
                failures.push(format!("{}: {err}", google_calendar_name(&calendar)));
                continue;
            }
        };

        let recurrence_exdates = google_recurring_exception_exdates(&events.events);
        let mut items =
            google_events_to_items(account, &calendar.id, &events.events, &recurrence_exdates);
        sort_imported_items(&mut items);

        let deleted = events
            .events
            .iter()
            .filter(|event| event.status.as_deref() == Some("cancelled"))
            .map(google_event_key)
            .collect();

        imported.push(ImportedGoogleCalendar {
            account_id: account.account_id.clone(),
            account_email: account.email.clone(),
            calendar_id: calendar.id.clone(),
            name: IMPORTED_GOOGLE_CALENDAR_SCHEME_NAME.to_string(),
            color_index: google_calendar_color_index(
                calendar.background_color.as_deref(),
                index % fallback_count,
            ),
            sync_token: events.sync_token,
            full_sync: events.full_sync,
            items,
            deleted,
            recurrence_exdates,
        });
    }

    Ok((imported, failures))
}

pub(crate) struct GoogleEventsSync {
    events: Vec<GoogleEvent>,
    sync_token: Option<String>,
    full_sync: bool,
}

pub(crate) fn list_google_calendars(
    access_token: &str,
) -> std::result::Result<Vec<GoogleCalendarListEntry>, GoogleApiError> {
    let mut page_token: Option<String> = None;
    let mut calendars = Vec::new();

    loop {
        let mut params = vec![
            ("maxResults", "250".to_string()),
            ("minAccessRole", "reader".to_string()),
        ];
        if let Some(token) = &page_token {
            params.push(("pageToken", token.clone()));
        }
        let url = with_query(GOOGLE_CALENDAR_LIST_URL, &params);
        let response: GoogleCalendarListResponse = google_get_json(&url, access_token)?;
        calendars.extend(
            response
                .items
                .into_iter()
                .filter(|calendar| calendar.deleted != Some(true) && calendar.hidden != Some(true)),
        );
        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    let visible = calendars
        .iter()
        .filter(|calendar| calendar.selected != Some(false) || calendar.primary == Some(true))
        .cloned()
        .collect::<Vec<_>>();
    if visible.is_empty() {
        Ok(calendars)
    } else {
        Ok(visible)
    }
}

pub(crate) fn list_google_events(
    access_token: &str,
    calendar_id: &str,
    sync_token: Option<String>,
) -> std::result::Result<GoogleEventsSync, GoogleApiError> {
    match list_google_events_once(access_token, calendar_id, sync_token.clone()) {
        Ok(events) => Ok(events),
        Err(err) if err.status == Some(410) && sync_token.is_some() => {
            google_oauth_log(format!(
                "events.list sync_token_expired calendar_id={calendar_id}; retrying full sync"
            ));
            list_google_events_once(access_token, calendar_id, None)
        }
        Err(err) => Err(err),
    }
}

pub(crate) fn list_google_events_once(
    access_token: &str,
    calendar_id: &str,
    sync_token: Option<String>,
) -> std::result::Result<GoogleEventsSync, GoogleApiError> {
    let base = format!(
        "{GOOGLE_EVENTS_BASE_URL}/{}/events",
        urlencoding::encode(calendar_id)
    );
    let mut page_token: Option<String> = None;
    let full_sync = sync_token.is_none();
    let mut events = Vec::new();
    let mut next_sync_token = sync_token.clone();

    loop {
        let mut params = vec![
            ("maxResults", "2500".to_string()),
            ("singleEvents", "false".to_string()),
        ];
        if let Some(token) = &sync_token {
            params.push(("syncToken", token.clone()));
            params.push(("showDeleted", "true".to_string()));
        } else {
            params.push(("showDeleted", "true".to_string()));
        }
        if let Some(token) = &page_token {
            params.push(("pageToken", token.clone()));
        }

        let url = with_query(&base, &params);
        let response: GoogleEventsResponse = google_get_json(&url, access_token)?;
        events.extend(response.items);
        if let Some(token) = response.next_sync_token {
            next_sync_token = Some(token);
        }
        page_token = response.next_page_token;
        if page_token.is_none() {
            break;
        }
    }

    Ok(GoogleEventsSync {
        events,
        sync_token: next_sync_token,
        full_sync,
    })
}

pub(crate) fn google_get_json<T: DeserializeOwned>(
    url: &str,
    access_token: &str,
) -> std::result::Result<T, GoogleApiError> {
    let auth = format!("Bearer {access_token}");
    let response = ureq::get(url)
        .set("Authorization", &auth)
        .call()
        .map_err(google_api_error)?;
    response.into_json::<T>().map_err(|err| GoogleApiError {
        status: None,
        message: format!("parse Google Calendar response: {err}"),
    })
}

pub(crate) fn google_api_error(err: ureq::Error) -> GoogleApiError {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            GoogleApiError {
                status: Some(status),
                message: format!("Google Calendar HTTP {status}: {body}"),
            }
        }
        ureq::Error::Transport(err) => GoogleApiError {
            status: None,
            message: format!("Google Calendar request failed: {err}"),
        },
    }
}

pub(crate) fn with_query(base: &str, params: &[(&str, String)]) -> String {
    if params.is_empty() {
        return base.to_string();
    }
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{query}")
}

pub(crate) fn google_http_error(err: ureq::Error) -> anyhow::Error {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            anyhow!(format_google_http_error(status, &body))
        }
        ureq::Error::Transport(err) => anyhow!("Google OAuth request failed: {err}"),
    }
}

pub(crate) fn format_google_http_error(status: u16, body: &str) -> String {
    if body.contains("client_secret is missing") {
        return format!(
            "Google OAuth HTTP {status}: {body}\n\nGoogle rejected the request because client_secret was missing. KnotQ expects {GOOGLE_OAUTH_CLIENT_SECRET_ENV} to be set at compile time and sends it with Google Desktop OAuth token requests."
        );
    }
    format!("Google OAuth HTTP {status}: {body}")
}

pub(crate) fn write_http_response(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>KnotQ</title></head><body>{body}</body></html>"
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    )
}

pub(crate) fn open_browser(url: &str) -> Result<()> {
    open_browser_with(url, webbrowser::open)
}

fn open_browser_with(url: &str, opener: impl FnOnce(&str) -> std::io::Result<()>) -> Result<()> {
    opener(url).context("open URL in default browser")
}

#[cfg(test)]
mod browser_command_tests {
    use std::cell::RefCell;
    use std::io;

    const AUTH_URL: &str = "https://www.knotq.com/signin?redirect_uri=http%3A%2F%2F127.0.0.1%3A43129&state=state&code_challenge=challenge&code_challenge_method=S256";

    #[test]
    fn opener_receives_the_complete_url_unchanged() {
        let received = RefCell::new(None);
        super::open_browser_with(AUTH_URL, |url| {
            *received.borrow_mut() = Some(url.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(received.into_inner().as_deref(), Some(AUTH_URL));
    }

    #[test]
    fn opener_errors_keep_browser_context() {
        let error = super::open_browser_with(AUTH_URL, |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "no handler"))
        })
        .unwrap_err();

        assert!(error.to_string().contains("open URL in default browser"));
    }

    #[test]
    fn hardened_browser_dependency_accepts_https_and_rejects_file_urls() {
        let mut options = webbrowser::BrowserOptions::new();
        options.with_dry_run(true);
        webbrowser::open_browser_with_options(webbrowser::Browser::Default, AUTH_URL, &options)
            .unwrap();
        assert!(webbrowser::open("file:///C:/Users/customer/Documents").is_err());
    }
}

pub(crate) fn random_token(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

pub(crate) fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub(crate) fn decode_id_token_claims(id_token: &str) -> Option<GoogleIdClaims> {
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}
