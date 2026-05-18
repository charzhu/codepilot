//! Agency builtin MCP discovery: `~/.agency/agency.toml` `[mcps.builtins]`.
//!
//! Each builtin entry is rewritten into a stdio invocation of
//! `agency mcp <name> [...flags] --transport stdio`. We require the
//! `agency` executable to be discoverable on `PATH` (or via the conventional
//! Windows install path) before emitting any entries; otherwise the spawned
//! commands would fail at connect time.

use std::collections::BTreeMap;
use std::path::PathBuf;

use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;

use crate::env::ExternalMcpEnv;
use crate::sources::SourceItem;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

/// Scan the Agency config and produce one [`SourceItem::Server`] per builtin.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let Some(home) = env.home_dir() else {
        return Vec::new();
    };
    let path = home.join(".agency").join("agency.toml");
    if !env.path_exists(&path) {
        return Vec::new();
    }
    let Some(agency_exe) = resolve_agency_exe(env) else {
        tracing::debug!("agency.toml found but agency executable not on PATH");
        return Vec::new();
    };
    let raw = match env.read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to read agency.toml");
            return Vec::new();
        }
    };
    let document: toml::Value = match toml::from_str(&raw) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to parse agency.toml");
            return Vec::new();
        }
    };
    let Some(builtins) = document
        .get("mcps")
        .and_then(|value| value.get("builtins"))
        .and_then(|value| value.as_table())
    else {
        return Vec::new();
    };
    let agency_exe_str = agency_exe.to_string_lossy().to_string();
    let mut out = Vec::new();
    for (name, value) in builtins {
        match build_server(&agency_exe_str, name, value) {
            Some(config) => {
                let fingerprint = crate::fingerprint::fingerprint(&config);
                out.push(SourceItem::Server(Box::new(DiscoveredMcpServer {
                    name: name.clone(),
                    config,
                    source: ExternalMcpSource::AgencyBuiltin,
                    origin_path: path.clone(),
                    fingerprint,
                })));
            }
            None => tracing::debug!(server = %name, "skipping agency builtin entry"),
        }
    }
    out
}

fn resolve_agency_exe(env: &dyn ExternalMcpEnv) -> Option<PathBuf> {
    if let Some(found) = env.which("agency") {
        return Some(found);
    }
    if let Some(appdata) = env.env_var("APPDATA") {
        let candidate = PathBuf::from(appdata)
            .join("agency")
            .join("CurrentVersion")
            .join("agency.exe");
        if env.path_exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn build_server(agency_exe: &str, name: &str, value: &toml::Value) -> Option<McpServerConfig> {
    let mut args = vec!["mcp".to_string(), name.to_string()];
    match value {
        toml::Value::Boolean(true) => {}
        toml::Value::Boolean(false) => return None,
        toml::Value::Table(table) => {
            for (key, val) in to_sorted_pairs(table) {
                if key == "type" {
                    continue;
                }
                let value_str = match val {
                    toml::Value::String(s) => s.clone(),
                    toml::Value::Integer(i) => i.to_string(),
                    toml::Value::Float(f) => f.to_string(),
                    toml::Value::Boolean(b) => b.to_string(),
                    _ => continue,
                };
                args.push(format!("--{}", key.replace('_', "-")));
                args.push(value_str);
            }
        }
        _ => return None,
    }
    args.push("--transport".to_string());
    args.push("stdio".to_string());
    Some(McpServerConfig {
        transport: McpServerTransportConfig::Stdio {
            command: agency_exe.to_string(),
            args,
            env: None,
            env_vars: Vec::new(),
            cwd: None,
        },
        experimental_environment: None,
        enabled: true,
        required: false,
        supports_parallel_tool_calls: false,
        disabled_reason: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: None,
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: std::collections::HashMap::new(),
    })
}

fn to_sorted_pairs(table: &toml::Table) -> BTreeMap<&String, &toml::Value> {
    table.iter().collect()
}
