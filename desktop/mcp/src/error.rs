use thiserror::Error;

/// Everything a tool call can go wrong with, in the shape MCP needs it.
///
/// Split by *who* has to fix it. A `Protocol`/`UnknownTool`/`InvalidParams`
/// failure is the client's bug and comes back as a JSON-RPC error, which is how
/// a client learns it called us wrong. Everything else is a perfectly
/// well-formed call that the workspace refused (the scheme is read-only, the id
/// no longer exists, someone else edited first), and those come back as a
/// *successful* `tools/call` carrying `isError: true` — the MCP convention for
/// "the tool ran and the answer is no", so the model reads the reason and can
/// act on it instead of the client treating it as a transport fault.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    #[error("{0}")]
    InvalidParams(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Conflict(String),
}

impl ToolError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidParams(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn refused(msg: impl Into<String>) -> Self {
        Self::Refused(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    /// Whether this is the client's fault (a JSON-RPC error) rather than a
    /// legitimate refusal the model should see and reason about.
    pub fn is_protocol_error(&self) -> bool {
        matches!(self, Self::UnknownTool(_) | Self::InvalidParams(_))
    }

    /// JSON-RPC error code, used only when `is_protocol_error`.
    pub fn rpc_code(&self) -> i64 {
        match self {
            Self::UnknownTool(_) => -32601,
            _ => -32602,
        }
    }

    /// Stable machine-readable tag, so a client can branch without parsing prose.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UnknownTool(_) => "unknown_tool",
            Self::InvalidParams(_) => "invalid_params",
            Self::NotFound(_) => "not_found",
            Self::Refused(_) => "refused",
            Self::Conflict(_) => "conflict",
        }
    }
}
