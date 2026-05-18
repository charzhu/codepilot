//! Shared helpers for loading `{ "mcpServers": { ... } }` style JSON files.

use std::collections::HashMap;
use std::path::Path;

use codex_config::McpServerConfig;
use codex_config::RawMcpServerConfig;
use serde::Deserialize;

use crate::env::ExternalMcpEnv;

/// Outcome for a single named entry parsed from a Claude/Copilot-style file.
pub(crate) enum ParsedEntry {
    Server(Box<McpServerConfig>),
    /// `{ "foo": false }` — explicitly disabled by this source.
    Disabled,
    /// Malformed entry; logged but not fatal.
    Invalid(String),
}

/// Outer container for `.mcp.json` / `mcp-config.json` files.
#[derive(Debug, Deserialize)]
pub(crate) struct McpServersFile {
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: HashMap<String, serde_json::Value>,
}

/// Load and parse a `{ "mcpServers": ... }` JSON file. Returns `Ok(None)`
/// when the file does not exist; `Err` is reserved for malformed JSON the
/// caller may want to log.
pub(crate) fn load_mcp_servers_file(
    env: &dyn ExternalMcpEnv,
    path: &Path,
) -> anyhow::Result<Option<HashMap<String, ParsedEntry>>> {
    if !env.path_exists(path) {
        return Ok(None);
    }
    let raw = env.read_to_string(path)?;
    let file: McpServersFile = serde_json::from_str(&raw)?;
    Ok(Some(parse_entries(file.mcp_servers)))
}

/// Convert raw JSON values into [`ParsedEntry`] without short-circuiting on a
/// single bad entry. Malformed entries are reported individually so the user
/// can fix them one at a time.
pub(crate) fn parse_entries(
    raw: HashMap<String, serde_json::Value>,
) -> HashMap<String, ParsedEntry> {
    let mut out = HashMap::with_capacity(raw.len());
    for (name, value) in raw {
        let entry = parse_entry(value);
        out.insert(name, entry);
    }
    out
}

fn parse_entry(value: serde_json::Value) -> ParsedEntry {
    if let serde_json::Value::Bool(false) = &value {
        return ParsedEntry::Disabled;
    }
    match serde_json::from_value::<RawMcpServerConfig>(value) {
        Ok(raw) => match McpServerConfig::try_from(raw) {
            Ok(config) => ParsedEntry::Server(Box::new(config)),
            Err(err) => ParsedEntry::Invalid(err),
        },
        Err(err) => ParsedEntry::Invalid(err.to_string()),
    }
}
