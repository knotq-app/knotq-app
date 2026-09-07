//! Explicit, reversible registration of KnotQ's bundled MCP bridge with AI
//! clients. Only KnotQ's own `knotq` entry is changed; existing client config
//! and other servers are preserved.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use toml_edit::{value, DocumentMut, Item, Value as TomlValue};
use uuid::Uuid;

const KNOTQ_SERVER_NAME: &str = "knotq";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum McpClient {
    ClaudeDesktop,
    ClaudeCode,
    Codex,
    Cursor,
}

impl McpClient {
    pub(crate) const ALL: [Self; 4] = [
        Self::ClaudeDesktop,
        Self::ClaudeCode,
        Self::Codex,
        Self::Cursor,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "Claude Desktop",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    fn format(self) -> ConfigFormat {
        match self {
            Self::Codex => ConfigFormat::Toml,
            Self::ClaudeDesktop | Self::ClaudeCode | Self::Cursor => ConfigFormat::Json,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigFormat {
    Json,
    Toml,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

/// Global client configuration path. Passing `home` in makes the platform map
/// testable without reading or mutating the developer's actual client configs.
fn config_path(
    client: McpClient,
    platform: Platform,
    home: &Path,
    app_data: Option<&Path>,
) -> PathBuf {
    match client {
        McpClient::ClaudeDesktop => match platform {
            Platform::Macos => home
                .join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json"),
            Platform::Windows => app_data
                .unwrap_or(home)
                .join("Claude")
                .join("claude_desktop_config.json"),
            Platform::Linux => match app_data {
                Some(config) => config.join("Claude").join("claude_desktop_config.json"),
                None => home
                    .join(".config")
                    .join("Claude")
                    .join("claude_desktop_config.json"),
            },
        },
        McpClient::ClaudeCode => home.join(".claude.json"),
        McpClient::Codex => home.join(".codex").join("config.toml"),
        McpClient::Cursor => home.join(".cursor").join("mcp.json"),
    }
}

fn current_home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("could not determine the current user's home directory")
}

fn current_app_data(platform: Platform, home: &Path) -> Option<PathBuf> {
    match platform {
        Platform::Windows => std::env::var_os("APPDATA").map(PathBuf::from),
        Platform::Linux => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| Some(home.join(".config"))),
        Platform::Macos => None,
    }
}

pub(crate) fn installed(client: McpClient) -> Result<bool> {
    let home = current_home()?;
    let platform = Platform::current();
    let app_data = current_app_data(platform, &home);
    let path = config_path(client, platform, &home, app_data.as_deref());
    contains_knotq_entry(&path, client.format())
}

pub(crate) fn install(client: McpClient, bridge: &Path, data_dir: &Path) -> Result<PathBuf> {
    if !bridge.is_file() {
        bail!(
            "the bundled MCP bridge was not found at {}",
            bridge.display()
        );
    }
    let home = current_home()?;
    let platform = Platform::current();
    let app_data = current_app_data(platform, &home);
    let path = config_path(client, platform, &home, app_data.as_deref());
    match client.format() {
        ConfigFormat::Json => install_json(&path, bridge, data_dir)?,
        ConfigFormat::Toml => install_toml(&path, bridge, data_dir)?,
    }
    Ok(path)
}

pub(crate) fn remove(client: McpClient) -> Result<PathBuf> {
    let home = current_home()?;
    let platform = Platform::current();
    let app_data = current_app_data(platform, &home);
    let path = config_path(client, platform, &home, app_data.as_deref());
    match client.format() {
        ConfigFormat::Json => remove_json(&path)?,
        ConfigFormat::Toml => remove_toml(&path)?,
    }
    Ok(path)
}

fn contains_knotq_entry(path: &Path, format: ConfigFormat) -> Result<bool> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    match format {
        ConfigFormat::Json => {
            let root: Value =
                serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
            Ok(root
                .get("mcpServers")
                .and_then(Value::as_object)
                .is_some_and(|servers| servers.contains_key(KNOTQ_SERVER_NAME)))
        }
        ConfigFormat::Toml => {
            let doc = text
                .parse::<DocumentMut>()
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(doc
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|servers| servers.get(KNOTQ_SERVER_NAME))
                .is_some_and(|entry| {
                    entry.is_table()
                        || entry
                            .as_value()
                            .and_then(TomlValue::as_inline_table)
                            .is_some()
                }))
        }
    }
}

fn install_json(path: &Path, bridge: &Path, data_dir: &Path) -> Result<()> {
    let mut root = read_json_object(path)?;
    let servers = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(servers) = servers.as_object_mut() else {
        bail!("{} has a non-object mcpServers setting", path.display());
    };
    servers.insert(
        KNOTQ_SERVER_NAME.to_string(),
        json!({ "command": bridge.to_string_lossy(), "env": { "KNOTQ_DATA_DIR": data_dir } }),
    );
    write_atomic(
        path,
        serde_json::to_string_pretty(&Value::Object(root))?.as_bytes(),
    )
}

fn remove_json(path: &Path) -> Result<()> {
    let mut root = read_json_object(path)?;
    if let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(KNOTQ_SERVER_NAME);
    }
    write_atomic(
        path,
        serde_json::to_string_pretty(&Value::Object(root))?.as_bytes(),
    )
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    serde_json::from_str::<Value>(&text)
        .with_context(|| format!("parse {}", path.display()))?
        .as_object()
        .cloned()
        .with_context(|| format!("{} must contain a JSON object", path.display()))
}

fn install_toml(path: &Path, bridge: &Path, data_dir: &Path) -> Result<()> {
    let text = read_or_empty(path)?;
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parse {}", path.display()))?;
    doc["mcp_servers"][KNOTQ_SERVER_NAME]["command"] = value(bridge.to_string_lossy().as_ref());
    doc["mcp_servers"][KNOTQ_SERVER_NAME]["env"]["KNOTQ_DATA_DIR"] =
        value(data_dir.to_string_lossy().as_ref());
    write_atomic(path, doc.to_string().as_bytes())
}

fn remove_toml(path: &Path) -> Result<()> {
    let text = read_or_empty(path)?;
    let mut doc = text
        .parse::<DocumentMut>()
        .with_context(|| format!("parse {}", path.display()))?;
    if let Some(Item::Table(table)) = doc.get_mut("mcp_servers") {
        table.remove(KNOTQ_SERVER_NAME);
    }
    write_atomic(path, doc.to_string().as_bytes())
}

fn read_or_empty(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

/// Same-directory temp + rename, so an interrupted install cannot truncate a
/// client's existing configuration.
fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("client configuration has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let name = path
        .file_name()
        .context("client configuration has no filename")?;
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        name.to_string_lossy(),
        Uuid::new_v4()
    ));
    fs::write(&temporary, body).with_context(|| format!("write {}", temporary.display()))?;
    // POSIX `rename` replaces an existing file atomically. Windows does not,
    // so remove only the already-parsed target immediately before the rename;
    // otherwise an Update operation would work on macOS/Linux but fail on a
    // second click on Windows.
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("replace {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_paths_cover_all_supported_platforms() {
        let home = Path::new("/home/alex");
        let app_data = Path::new("/config");
        assert_eq!(
            config_path(McpClient::ClaudeDesktop, Platform::Macos, home, None),
            home.join("Library/Application Support/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(
                McpClient::ClaudeDesktop,
                Platform::Windows,
                home,
                Some(app_data)
            ),
            app_data.join("Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(McpClient::Cursor, Platform::Linux, home, None),
            home.join(".cursor/mcp.json")
        );
        assert_eq!(
            config_path(McpClient::Codex, Platform::Linux, home, None),
            home.join(".codex/config.toml")
        );
    }

    #[test]
    fn json_install_preserves_other_servers_and_remove_reverses_it() {
        let root = std::env::temp_dir().join(format!("knotq-mcp-client-{}", Uuid::new_v4()));
        let path = root.join("mcp.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"other"}},"theme":"dark"}"#,
        )
        .unwrap();
        let bridge = root.join("knotq-mcp");
        fs::write(&bridge, []).unwrap();

        install_json(&path, &bridge, &root).unwrap();
        install_json(&path, &bridge, &root).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["theme"], "dark");
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        assert_eq!(
            value["mcpServers"]["knotq"]["command"],
            bridge.to_string_lossy().as_ref()
        );

        remove_json(&path).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["mcpServers"].get("knotq").is_none());
        assert_eq!(value["mcpServers"]["other"]["command"], "other");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn toml_install_preserves_other_servers_and_remove_reverses_it() {
        let root = std::env::temp_dir().join(format!("knotq-mcp-client-{}", Uuid::new_v4()));
        let path = root.join("config.toml");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            "model = \"gpt\"\n\n[mcp_servers.other]\ncommand = \"other\"\n",
        )
        .unwrap();
        let bridge = root.join("knotq-mcp");
        fs::write(&bridge, []).unwrap();

        install_toml(&path, &bridge, &root).unwrap();
        install_toml(&path, &bridge, &root).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("model = \"gpt\""));
        assert!(text.contains("[mcp_servers.other]"));
        let document = text.parse::<DocumentMut>().unwrap();
        assert_eq!(
            document["mcp_servers"][KNOTQ_SERVER_NAME]["command"].as_str(),
            Some(bridge.to_string_lossy().as_ref())
        );

        remove_toml(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mcp_servers.other]"));
        assert!(!contains_knotq_entry(&path, ConfigFormat::Toml).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_toml_server_table_is_not_a_panic() {
        let root = std::env::temp_dir().join(format!("knotq-mcp-client-{}", Uuid::new_v4()));
        let path = root.join("config.toml");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, "model = \"gpt\"\n").unwrap();

        assert!(!contains_knotq_entry(&path, ConfigFormat::Toml).unwrap());
        remove_toml(&path).unwrap();
        assert!(!contains_knotq_entry(&path, ConfigFormat::Toml).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
