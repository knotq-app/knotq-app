//! Shared helpers used by both the encoder and decoder: XML 1.0 character
//! sanitization and RFC 3339 datetime conversion.

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use std::borrow::Cow;

/// Whether `c` is a legal XML 1.0 character. Control characters (other than tab,
/// newline, and CR) are forbidden and cannot be represented even via numeric
/// escapes, so they must be removed rather than encoded.
pub(super) fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// Drop characters illegal in XML 1.0. Without this, a pasted/imported control
/// character would be written verbatim and then fail to parse on the next load,
/// silently losing the whole scheme. Borrows when the input is already clean.
pub(super) fn strip_invalid_xml_chars(text: &str) -> Cow<'_, str> {
    if text.chars().all(is_xml_char) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.chars().filter(|&c| is_xml_char(c)).collect())
    }
}

pub(super) fn encode_datetime(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn parse_datetime(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}
