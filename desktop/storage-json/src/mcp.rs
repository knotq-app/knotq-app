//! The MCP endpoint descriptor: how a client finds the running server.
//!
//! The port and the bearer token both live here rather than in `settings.json`,
//! for two different reasons. The port because the *configured* port and the
//! *bound* port can differ — something else may hold the default — and a client
//! needs the one that is actually listening. The token because it is a
//! credential: keeping it out of the settings file keeps it out of anything
//! that copies, syncs, or displays settings, and lets it be regenerated
//! without rewriting a file the app is holding open.
//!
//! The file is rewritten on every start and removed when the server stops, so
//! its presence means "a server is listening right now".

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::files::write_atomic;
use crate::paths::data_dir;

pub fn mcp_endpoint_path() -> PathBuf {
    data_dir().join("mcp-endpoint.json")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpEndpoint {
    /// Full URL a client should POST JSON-RPC to.
    pub url: String,
    pub port: u16,
    /// Bearer token. Any process that can read this file can drive the server,
    /// which is the same trust boundary as being able to read the workspace
    /// files sitting beside it.
    pub token: String,
    /// Whether writes are currently refused, so a client can say so up front.
    pub read_only: bool,
    pub protocol_version: String,
}

pub fn save_mcp_endpoint(endpoint: &McpEndpoint) -> Result<()> {
    let path = mcp_endpoint_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create data dir for the MCP endpoint file")?;
    }
    let body = serde_json::to_vec_pretty(endpoint).context("serialize the MCP endpoint")?;
    write_atomic(&path, &body).context("write the MCP endpoint file")?;
    restrict_to_owner(&path);
    Ok(())
}

pub fn load_mcp_endpoint() -> Option<McpEndpoint> {
    let body = std::fs::read(mcp_endpoint_path()).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Remove the descriptor. Called when the server stops, so a stale file never
/// points a client at a port nothing is listening on.
pub fn clear_mcp_endpoint() {
    let path = mcp_endpoint_path();
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("could not remove {}: {e}", path.display()),
    }
}

/// Owner-only permissions, best effort. The file holds a credential; on a
/// shared machine the default umask is not enough.
#[cfg(unix)]
fn restrict_to_owner(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        eprintln!("could not restrict permissions on {}: {e}", path.display());
    }
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &std::path::Path) {
    // Windows inherits the user-profile ACL, which is already owner-scoped.
}
