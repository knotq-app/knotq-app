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
    let code = match wait_for_oauth_code(
        &listener,
        &state,
        StdDuration::from_secs(120),
        "Google Calendar is connected. You can close this tab and return to KnotQ.",
        "Google Calendar connection failed. You can close this tab and return to KnotQ.",
        Some(cancel_token),
    ) {
        Ok(code) => {
            google_oauth_log("oauth.callback ok");
            code
        }
        Err(err) => {
            google_oauth_log(format!("oauth.callback failed: {err:#}"));
            return Err(err);
        }
    };
    google_oauth_log("oauth.exchange start");
    let token = match exchange_auth_code(&config, &redirect_uri, &code, &code_verifier) {
        Ok(token) => {
            google_oauth_log("oauth.exchange ok");
            token
        }
        Err(err) => {
            google_oauth_log(format!("oauth.exchange failed: {err:#}"));
            return Err(err);
        }
    };
    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("Google did not return a refresh token"))?;
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
        scope: token.scope.unwrap_or(scope),
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
    let started = Instant::now();
    while started.elapsed() < timeout {
        if cancel_token.is_some_and(|cancel_token| cancel_token.load(Ordering::SeqCst)) {
            bail!(GOOGLE_OAUTH_CALLBACK_CANCELLED);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = read_oauth_callback(&mut stream, expected_state);
                let body = if result.is_ok() {
                    success_body
                } else {
                    failure_body
                };
                let _ = write_http_response(&mut stream, body);
                return result;
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(StdDuration::from_millis(100));
            }
            Err(err) => return Err(err).context("accept OAuth callback"),
        }
    }
    bail!(GOOGLE_OAUTH_CALLBACK_TIMEOUT)
}

pub(crate) fn read_oauth_callback(stream: &mut TcpStream, expected_state: &str) -> Result<String> {
    let mut buffer = [0u8; 4096];
    let len = stream.read(&mut buffer).context("read OAuth callback")?;
    let request = String::from_utf8_lossy(&buffer[..len]);
    let request_target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid OAuth callback request"))?;
    let params = query_params(request_target)?;
    if params.get("state").map(String::as_str) != Some(expected_state) {
        bail!("Google OAuth returned an unexpected state");
    }
    if let Some(error) = params.get("error") {
        bail!("Google OAuth error: {error}");
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow!("Google OAuth callback did not include a code"))
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
    Ok(())
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

pub(crate) fn google_desktop_client_secret_form_field(config: &GoogleOAuthConfig) -> (&'static str, String) {
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
    account: &GoogleOAuthAccount,
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
            calendars
        }
        Err(err) => {
            google_oauth_log(format!(
                "calendar_list failed account={}: {err:#}",
                google_account_label(account)
            ));
            return Err(err);
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
        let events = match list_google_events(&account.access_token, &calendar.id, sync_token) {
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

        let mut items = events
            .events
            .iter()
            .filter_map(|event| google_event_to_item(account, &calendar.id, event))
            .collect::<Vec<_>>();
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
        });
    }

    Ok((imported, failures))
}

pub(crate) struct GoogleEventsSync {
    events: Vec<GoogleEvent>,
    sync_token: Option<String>,
    full_sync: bool,
}

pub(crate) fn list_google_calendars(access_token: &str) -> Result<Vec<GoogleCalendarListEntry>> {
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
) -> Result<GoogleEventsSync> {
    match list_google_events_once(access_token, calendar_id, sync_token.clone()) {
        Ok(events) => Ok(events),
        Err(err) if err.status == Some(410) && sync_token.is_some() => {
            google_oauth_log(format!(
                "events.list sync_token_expired calendar_id={calendar_id}; retrying full sync"
            ));
            Ok(list_google_events_once(access_token, calendar_id, None)?)
        }
        Err(err) => Err(anyhow!(err)),
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
            params.push(("showDeleted", "false".to_string()));
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
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };

    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };

    command.spawn().context("open URL in browser")?;
    Ok(())
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
