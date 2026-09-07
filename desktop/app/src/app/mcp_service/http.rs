//! HTTP framing for the MCP endpoint, as pure functions over bytes.
//!
//! Hand-rolled rather than pulled from a framework, matching the loopback OAuth
//! listener next door: the surface is one POST on one path from one client, and
//! a web framework would add a large dependency and an async runtime to the
//! desktop binary to serve it.
//!
//! Keeping the parsing pure is what lets the refusal paths — the ones that are
//! load-bearing for security and hardest to reach through a socket — be
//! covered by ordinary unit tests.

use std::collections::HashMap;

/// Cap on a single request body. Every legitimate call is a small JSON-RPC
/// object; without a ceiling a local process could make the app allocate until
/// it dies.
pub(crate) const MAX_BODY_BYTES: usize = 1024 * 1024;

/// The path the server answers on. Anything else is a 404, so a browser that
/// wanders onto the port gets nothing interesting.
pub(crate) const MCP_PATH: &str = "/mcp";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn content_length(&self) -> Option<usize> {
        self.header("content-length")?.trim().parse().ok()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// The head is not complete yet — read more from the socket.
    Incomplete,
    Malformed(&'static str),
    TooLarge,
}

/// Parse a request whose head is complete. `body` may still be short; the
/// caller tops it up using [`HttpRequest::content_length`].
pub(crate) fn parse_request(buffer: &[u8]) -> Result<HttpRequest, ParseError> {
    let text = String::from_utf8_lossy(buffer);
    let Some(head_end) = text.find("\r\n\r\n").map(|i| (i, 4)).or_else(|| {
        // Tolerate bare LF line endings: hand-written clients and some shells
        // produce them, and there is nothing to gain by refusing.
        text.find("\n\n").map(|i| (i, 2))
    }) else {
        if buffer.len() > MAX_BODY_BYTES {
            return Err(ParseError::TooLarge);
        }
        return Err(ParseError::Incomplete);
    };
    let (head, body_offset) = head_end;
    let head_text = &text[..head];
    let mut lines = head_text.lines();
    let request_line = lines.next().ok_or(ParseError::Malformed("empty request"))?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or(ParseError::Malformed("no method"))?
        .to_string();
    let target = parts
        .next()
        .ok_or(ParseError::Malformed("no request target"))?;
    // Strip any query string: this endpoint takes no query parameters, and
    // matching on the raw target would make `/mcp?x=1` a 404.
    let path = target.split('?').next().unwrap_or(target).to_string();

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ParseError::Malformed("header without a colon"));
        };
        // Later duplicates win, matching what a real server does, and header
        // names are case-insensitive per RFC 9110.
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let body = text[head + body_offset..].to_string();
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

/// Why a request was turned away before it reached the workspace.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Rejection {
    NotFound,
    MethodNotAllowed,
    /// Missing or wrong bearer token.
    Unauthorized,
    /// An `Origin` header that is not a loopback page.
    ///
    /// This is the DNS-rebinding defence the MCP spec requires of local HTTP
    /// servers: without it, a page on any website can be made to resolve a
    /// hostname to 127.0.0.1 and then POST to this port from the user's own
    /// browser, with the browser attaching nothing the server could use to tell
    /// it apart from a real client.
    ForbiddenOrigin,
    TooLarge,
}

impl Rejection {
    pub fn status(&self) -> (u16, &'static str) {
        match self {
            Self::NotFound => (404, "Not Found"),
            Self::MethodNotAllowed => (405, "Method Not Allowed"),
            Self::Unauthorized => (401, "Unauthorized"),
            Self::ForbiddenOrigin => (403, "Forbidden"),
            Self::TooLarge => (413, "Payload Too Large"),
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::NotFound => "no such endpoint",
            Self::MethodNotAllowed => "this endpoint accepts POST",
            Self::Unauthorized => "a valid bearer token is required",
            Self::ForbiddenOrigin => "requests from a browser origin are refused",
            Self::TooLarge => "request body too large",
        }
    }
}

/// Everything that must hold before a request is allowed to touch the workspace.
pub(crate) fn admit(request: &HttpRequest, token: &str) -> Result<(), Rejection> {
    if request.path != MCP_PATH {
        return Err(Rejection::NotFound);
    }
    if !request.method.eq_ignore_ascii_case("POST") {
        return Err(Rejection::MethodNotAllowed);
    }
    if !origin_is_acceptable(request.header("origin")) {
        return Err(Rejection::ForbiddenOrigin);
    }
    if !token_matches(request.header("authorization"), token) {
        return Err(Rejection::Unauthorized);
    }
    Ok(())
}

/// A non-browser client sends no `Origin` at all, which is fine. A browser
/// always sends one, and the only browser origin that could legitimately be
/// talking to a loopback server is another loopback page.
pub(crate) fn origin_is_acceptable(origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let origin = origin.trim();
    if origin.is_empty() || origin == "null" {
        return true;
    }
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or(rest);
    // Strip the port, and the brackets an IPv6 literal carries.
    let host = match host.rsplit_once(':') {
        // Only treat the tail as a port if it is one; `[::1]` has colons of its own.
        Some((head, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => head,
        _ => host,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

/// Constant-time-ish bearer comparison.
///
/// Not constant time in the cryptographic sense — a local attacker has better
/// options than timing this — but it does avoid the early return that would
/// leak the token's length and prefix to a caller that can measure it.
pub(crate) fn token_matches(header: Option<&str>, expected: &str) -> bool {
    let Some(header) = header else {
        return false;
    };
    let Some(presented) = header
        .trim()
        .strip_prefix("Bearer ")
        .or_else(|| header.trim().strip_prefix("bearer "))
    else {
        return false;
    };
    let presented = presented.trim().as_bytes();
    let expected = expected.as_bytes();
    if presented.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in presented.iter().zip(expected.iter()) {
        difference |= a ^ b;
    }
    difference == 0
}

pub(crate) fn json_response(status: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.as_bytes().len(),
    )
}

pub(crate) fn rejection_response(rejection: &Rejection) -> String {
    let (status, reason) = rejection.status();
    let body = serde_json::json!({ "error": rejection.message() }).to_string();
    json_response(status, reason, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(raw: &str) -> HttpRequest {
        parse_request(raw.as_bytes()).expect("should parse")
    }

    const TOKEN: &str = "s3cret-token";

    fn post(headers: &str, body: &str) -> HttpRequest {
        request(&format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    #[test]
    fn a_head_that_has_not_arrived_yet_asks_for_more_rather_than_failing() {
        assert_eq!(
            parse_request(b"POST /mcp HTTP/1.1\r\nHost: 127."),
            Err(ParseError::Incomplete)
        );
    }

    #[test]
    fn headers_are_matched_without_regard_to_case() {
        let r = post("AUTHORIZATION: Bearer x\r\n", "{}");
        assert_eq!(r.header("authorization"), Some("Bearer x"));
        assert_eq!(r.header("Authorization"), Some("Bearer x"));
    }

    #[test]
    fn a_query_string_does_not_change_which_endpoint_was_asked_for() {
        let r = request("POST /mcp?session=1 HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(r.path, "/mcp");
    }

    #[test]
    fn bare_lf_line_endings_are_accepted() {
        let r = parse_request(b"POST /mcp HTTP/1.1\nHost: x\n\n{}").expect("should parse");
        assert_eq!(r.path, "/mcp");
        assert_eq!(r.body, "{}");
    }

    #[test]
    fn only_post_to_the_mcp_path_is_admitted() {
        assert_eq!(admit(&post("Authorization: Bearer s3cret-token\r\n", "{}"), TOKEN), Ok(()));

        let wrong_path = request("POST /admin HTTP/1.1\r\nAuthorization: Bearer s3cret-token\r\n\r\n");
        assert_eq!(admit(&wrong_path, TOKEN), Err(Rejection::NotFound));

        let wrong_method =
            request("GET /mcp HTTP/1.1\r\nAuthorization: Bearer s3cret-token\r\n\r\n");
        assert_eq!(admit(&wrong_method, TOKEN), Err(Rejection::MethodNotAllowed));
    }

    #[test]
    fn a_request_without_the_token_is_refused() {
        assert_eq!(admit(&post("", "{}"), TOKEN), Err(Rejection::Unauthorized));
    }

    #[test]
    fn a_request_with_the_wrong_token_is_refused() {
        assert_eq!(
            admit(&post("Authorization: Bearer wrong\r\n", "{}"), TOKEN),
            Err(Rejection::Unauthorized)
        );
        // A prefix of the real token must not be enough.
        assert_eq!(
            admit(&post("Authorization: Bearer s3cret\r\n", "{}"), TOKEN),
            Err(Rejection::Unauthorized)
        );
    }

    #[test]
    fn the_scheme_keyword_must_be_present_and_is_case_insensitive() {
        assert!(!token_matches(Some("s3cret-token"), TOKEN));
        assert!(token_matches(Some("Bearer s3cret-token"), TOKEN));
        assert!(token_matches(Some("bearer s3cret-token"), TOKEN));
    }

    /// The DNS-rebinding defence. A page on any site can be made to resolve a
    /// name to 127.0.0.1 and POST here from the user's browser; the browser
    /// attaches its real origin, and that is the only thing distinguishing it.
    #[test]
    fn a_browser_page_from_the_web_is_refused_even_with_a_valid_token() {
        let attacker = post(
            "Origin: https://evil.example\r\nAuthorization: Bearer s3cret-token\r\n",
            "{}",
        );
        assert_eq!(admit(&attacker, TOKEN), Err(Rejection::ForbiddenOrigin));
    }

    #[test]
    fn origin_is_checked_before_the_token_so_a_stolen_token_does_not_help_a_browser() {
        let attacker = post("Origin: https://evil.example\r\n", "{}");
        // No token at all, but the origin is what it is turned away for — the
        // check must not be reachable-around by fixing the other half.
        assert_eq!(admit(&attacker, TOKEN), Err(Rejection::ForbiddenOrigin));
    }

    #[test]
    fn loopback_origins_are_allowed_and_everything_else_is_not() {
        for allowed in [
            None,
            Some(""),
            Some("null"),
            Some("http://localhost"),
            Some("http://localhost:3000"),
            Some("http://127.0.0.1:8080"),
            Some("https://127.0.0.1"),
            Some("http://[::1]:1234"),
        ] {
            assert!(origin_is_acceptable(allowed), "{allowed:?} should be allowed");
        }
        for refused in [
            Some("https://evil.example"),
            Some("http://localhost.evil.example"),
            Some("http://127.0.0.1.evil.example"),
            Some("file://"),
            Some("http://0.0.0.0"),
            Some("http://10.0.0.1"),
        ] {
            assert!(!origin_is_acceptable(refused), "{refused:?} should be refused");
        }
    }

    /// `localhost.evil.example` ends with nothing loopback about it, but a
    /// prefix or `contains` check would wave it through.
    #[test]
    fn a_hostname_that_merely_starts_with_localhost_is_not_loopback() {
        assert!(!origin_is_acceptable(Some("http://localhost.evil.example")));
        assert!(!origin_is_acceptable(Some("http://127.0.0.1.evil.example:80")));
    }

    #[test]
    fn responses_carry_an_accurate_byte_length_for_non_ascii_bodies() {
        let body = r#"{"text":"café ✅"}"#;
        let response = json_response(200, "OK", body);
        let declared: usize = response
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // Bytes, not characters — a char count here truncates the body.
        assert_eq!(declared, body.as_bytes().len());
        assert!(declared > body.chars().count());
    }

    #[test]
    fn a_rejection_renders_as_json_with_its_status() {
        let response = rejection_response(&Rejection::Unauthorized);
        assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        assert!(response.contains("bearer token"));
    }
}
