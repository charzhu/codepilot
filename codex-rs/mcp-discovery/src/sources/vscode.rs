//! VS Code project MCP discovery: `./.vscode/mcp.json`.
//!
//! The VS Code format wraps servers under the top-level key `servers`
//! (not `mcpServers`) and supports `${workspaceFolder}` and `${env:NAME}`
//! placeholders that we expand before normalization.

use std::collections::HashMap;
use std::path::Path;

use codex_config::McpServerConfig;
use codex_config::RawMcpServerConfig;
use codex_utils_path_uri::LegacyAppPathString;
use serde::Deserialize;

use crate::env::ExternalMcpEnv;
use crate::normalize::expand_vscode_vars;
use crate::sources::SourceItem;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

#[derive(Debug, Deserialize)]
struct VscodeMcpFile {
    #[serde(default)]
    servers: HashMap<String, VscodeServer>,
}

#[derive(Debug, Deserialize)]
struct VscodeServer {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    http_headers: Option<HashMap<String, String>>,
}

/// Scan `<cwd>/.vscode/mcp.json` and emit normalized server entries.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let path = env.cwd().join(".vscode").join("mcp.json");
    if !env.path_exists(&path) {
        return Vec::new();
    }
    let raw = match env.read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "failed to read VS Code .vscode/mcp.json",
            );
            return Vec::new();
        }
    };
    let parsed: VscodeMcpFile = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                %err,
                "invalid JSON in VS Code .vscode/mcp.json",
            );
            return Vec::new();
        }
    };
    let cwd = env.cwd().to_path_buf();
    let mut out = Vec::new();
    for (name, server) in parsed.servers {
        match build_raw(server, &cwd, env) {
            Ok(raw_config) => match McpServerConfig::try_from(raw_config) {
                Ok(config) => {
                    let fingerprint = crate::fingerprint::fingerprint(&config);
                    out.push(SourceItem::Server(Box::new(DiscoveredMcpServer {
                        name,
                        config,
                        source: ExternalMcpSource::VsCode,
                        origin_path: path.clone(),
                        fingerprint,
                    })));
                }
                Err(err) => tracing::warn!(
                    server = %name,
                    path = %path.display(),
                    %err,
                    "invalid VS Code MCP entry",
                ),
            },
            Err(err) => tracing::warn!(
                server = %name,
                path = %path.display(),
                %err,
                "invalid VS Code MCP entry",
            ),
        }
    }
    out
}

fn build_raw(
    server: VscodeServer,
    cwd: &Path,
    env: &dyn ExternalMcpEnv,
) -> Result<RawMcpServerConfig, String> {
    let expander = |value: &str| expand_vscode_vars(value, cwd, |name| env.env_var(name));
    let transport_type = server.r#type.as_deref().unwrap_or("stdio");
    let mut raw = RawMcpServerConfig {
        command: None,
        args: None,
        env: None,
        env_vars: None,
        cwd: None,
        http_headers: None,
        env_http_headers: None,
        url: None,
        bearer_token: None,
        bearer_token_env_var: None,
        environment_id: None,
        auth: None,
        startup_timeout_sec: None,
        startup_timeout_ms: None,
        tool_timeout_sec: None,
        enabled: None,
        required: None,
        supports_parallel_tool_calls: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: None,
        scopes: None,
        oauth: None,
        oauth_resource: None,
        _name: None,
        tools: None,
    };
    match transport_type {
        "stdio" => {
            let command = server
                .command
                .ok_or_else(|| "stdio transport requires a command".to_string())?;
            raw.command = Some(expander(&command));
            raw.args = server
                .args
                .map(|args| args.into_iter().map(|arg| expander(&arg)).collect());
            raw.env = server.env.map(|env_map| {
                env_map
                    .into_iter()
                    .map(|(key, value)| (key, expander(&value)))
                    .collect()
            });
            raw.cwd = server
                .cwd
                .map(|value| LegacyAppPathString::from_path(Path::new(&expander(&value))));
        }
        "http" | "sse" | "streamable-http" | "streamable_http" => {
            let url = server
                .url
                .ok_or_else(|| format!("{transport_type} transport requires a url"))?;
            raw.url = Some(expander(&url));
            raw.http_headers = server.http_headers.map(|headers| {
                headers
                    .into_iter()
                    .map(|(key, value)| (key, expander(&value)))
                    .collect()
            });
        }
        other => return Err(format!("unsupported transport type: {other}")),
    }
    Ok(raw)
}
