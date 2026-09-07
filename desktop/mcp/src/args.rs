//! Typed accessors over a tool call's `arguments` object.
//!
//! Every one of these produces a `ToolError::InvalidParams` naming the argument
//! rather than a generic parse failure — a model that gets "`scheme_id` is not
//! a valid id" fixes its next call, one that gets "invalid request" retries the
//! same thing forever.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::ToolError;

pub struct Args<'a> {
    value: &'a Value,
}

const EMPTY: Value = Value::Null;

impl<'a> Args<'a> {
    pub fn new(value: Option<&'a Value>) -> Self {
        Self {
            value: value.unwrap_or(&EMPTY),
        }
    }

    fn get(&self, name: &str) -> Option<&'a Value> {
        self.value
            .get(name)
            .filter(|v| !v.is_null())
    }

    /// True when the caller mentioned the key at all — including explicitly as
    /// `null`. Update tools need this to tell "leave this field alone" (absent)
    /// from "clear this field" (present and null).
    pub fn mentions(&self, name: &str) -> bool {
        self.value.get(name).is_some()
    }

    pub fn opt_str(&self, name: &str) -> Result<Option<&'a str>, ToolError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err(ToolError::invalid(format!("`{name}` must be a string"))),
        }
    }

    pub fn req_str(&self, name: &str) -> Result<&'a str, ToolError> {
        self.opt_str(name)?
            .ok_or_else(|| ToolError::invalid(format!("`{name}` is required")))
    }

    pub fn opt_bool(&self, name: &str) -> Result<Option<bool>, ToolError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(ToolError::invalid(format!("`{name}` must be true or false"))),
        }
    }

    pub fn bool_or(&self, name: &str, default: bool) -> Result<bool, ToolError> {
        Ok(self.opt_bool(name)?.unwrap_or(default))
    }

    pub fn opt_u64(&self, name: &str) -> Result<Option<u64>, ToolError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Number(n)) => n
                .as_u64()
                .ok_or_else(|| ToolError::invalid(format!("`{name}` must be a whole number >= 0")))
                .map(Some),
            Some(_) => Err(ToolError::invalid(format!("`{name}` must be a number"))),
        }
    }

    pub fn opt_usize(&self, name: &str) -> Result<Option<usize>, ToolError> {
        Ok(self.opt_u64(name)?.map(|n| n as usize))
    }

    /// A bounded count. Clamps rather than erroring on an over-large value:
    /// asking for 10,000 upcoming items is a reasonable thing for a model to
    /// try, and refusing it teaches nothing that silently capping does not.
    pub fn limit(&self, name: &str, default: usize, max: usize) -> Result<usize, ToolError> {
        let requested = self.opt_usize(name)?.unwrap_or(default);
        Ok(requested.clamp(1, max))
    }

    pub fn opt_u8(&self, name: &str, max: u8) -> Result<Option<u8>, ToolError> {
        match self.opt_u64(name)? {
            None => Ok(None),
            Some(n) if n <= u64::from(max) => Ok(Some(n as u8)),
            Some(_) => Err(ToolError::invalid(format!(
                "`{name}` must be between 0 and {max}"
            ))),
        }
    }

    pub fn opt_string_list(&self, name: &str) -> Result<Option<Vec<String>>, ToolError> {
        match self.get(name) {
            None => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| ToolError::invalid(format!("`{name}` must be strings")))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(ToolError::invalid(format!("`{name}` must be an array"))),
        }
    }

    /// An id newtype, parsed from its UUID spelling.
    pub fn opt_id<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, ToolError> {
        match self.opt_str(name)? {
            None => Ok(None),
            Some(s) => s.parse::<T>().map(Some).map_err(|_| {
                ToolError::invalid(format!(
                    "`{name}` is not a valid id — pass one returned by a read tool, verbatim"
                ))
            }),
        }
    }

    pub fn req_id<T: std::str::FromStr>(&self, name: &str) -> Result<T, ToolError> {
        self.opt_id(name)?
            .ok_or_else(|| ToolError::invalid(format!("`{name}` is required")))
    }

    /// An instant. RFC 3339 only, and the offset is required — a bare
    /// `2026-09-01T09:00:00` is ambiguous, and guessing a zone for it is how an
    /// agent silently schedules something for the wrong hour.
    pub fn opt_datetime(&self, name: &str) -> Result<Option<DateTime<Utc>>, ToolError> {
        match self.opt_str(name)? {
            None => Ok(None),
            Some(s) => DateTime::parse_from_rfc3339(s)
                .map(|dt| Some(dt.with_timezone(&Utc)))
                .map_err(|_| {
                    ToolError::invalid(format!(
                        "`{name}` must be an RFC 3339 timestamp including an offset, \
                         e.g. 2026-09-01T09:00:00Z or 2026-09-01T09:00:00-04:00"
                    ))
                }),
        }
    }

    pub fn req_datetime(&self, name: &str) -> Result<DateTime<Utc>, ToolError> {
        self.opt_datetime(name)?
            .ok_or_else(|| ToolError::invalid(format!("`{name}` is required")))
    }
}
