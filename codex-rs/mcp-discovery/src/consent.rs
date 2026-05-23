//! Consent bookkeeping for externally-discovered MCP servers.
//!
//! This module never prompts. It owns the on-disk consent file at
//! `<codex_home>/mcp-consent.json` and exposes [`ConsentStore::decide`] so
//! the higher layers (CLI/TUI) can ask "is this server approved?" without
//! reasoning about file formats.

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::types::DiscoveredMcpServer;

/// Result of a consent lookup. The TUI/CLI is expected to translate
/// `Pending` into an interactive prompt; the discovery crate itself stays
/// headless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    /// User explicitly approved this server (or it is trusted by source).
    Approved,
    /// User explicitly denied this server.
    Denied,
    /// No prior decision; embedder must prompt or fall back to a default.
    Pending,
}

/// Persistent consent record. Stored at `<codex_home>/mcp-consent.json`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    #[serde(default)]
    pub approved: Vec<String>,
    #[serde(default)]
    pub denied: Vec<String>,
    /// When true, every newly discovered server is treated as approved.
    #[serde(default)]
    pub auto_approve: bool,
}

/// File-backed consent store. Reads on construction, writes on every mutation.
#[derive(Debug, Clone)]
pub struct ConsentStore {
    path: PathBuf,
    record: ConsentRecord,
}

impl ConsentStore {
    /// Build a new store for `<codex_home>/mcp-consent.json`. If the file
    /// does not exist or is malformed, an empty record is used.
    pub fn load(codex_home: &std::path::Path) -> Self {
        let path = codex_home.join("mcp-consent.json");
        let record = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self { path, record }
    }

    /// In-memory constructor for tests. The store will only persist when the
    /// caller has write access to `path`.
    pub fn with_record(path: PathBuf, record: ConsentRecord) -> Self {
        Self { path, record }
    }

    pub fn record(&self) -> &ConsentRecord {
        &self.record
    }

    /// Decide whether a discovered server should be connected. Trusted sources
    /// (currently `Own`) are always approved regardless of consent state.
    pub fn decide(&self, server: &DiscoveredMcpServer) -> ConsentDecision {
        if server.source.trusted_by_default() {
            return ConsentDecision::Approved;
        }
        if self.record.denied.iter().any(|name| name == &server.name) {
            return ConsentDecision::Denied;
        }
        if self.record.auto_approve {
            return ConsentDecision::Approved;
        }
        if self.record.approved.iter().any(|name| name == &server.name) {
            return ConsentDecision::Approved;
        }
        ConsentDecision::Pending
    }

    /// Approve `server_name`, removing it from the denied list if present.
    /// Persists the change to disk.
    pub fn approve(&mut self, server_name: &str) -> std::io::Result<()> {
        self.record.denied.retain(|name| name != server_name);
        if !self.record.approved.iter().any(|name| name == server_name) {
            self.record.approved.push(server_name.to_string());
        }
        self.persist()
    }

    /// Deny `server_name`, removing it from the approved list if present.
    pub fn deny(&mut self, server_name: &str) -> std::io::Result<()> {
        self.record.approved.retain(|name| name != server_name);
        if !self.record.denied.iter().any(|name| name == server_name) {
            self.record.denied.push(server_name.to_string());
        }
        self.persist()
    }

    /// Toggle the "auto-approve everything" flag and persist.
    pub fn set_auto_approve(&mut self, enabled: bool) -> std::io::Result<()> {
        self.record.auto_approve = enabled;
        self.persist()
    }

    fn persist(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(&self.record)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(&self.path, body)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use codex_config::McpServerConfig;
    use codex_config::McpServerTransportConfig;
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;
    use crate::types::ExternalMcpSource;

    fn sample_server(name: &str, source: ExternalMcpSource) -> DiscoveredMcpServer {
        DiscoveredMcpServer {
            name: name.to_string(),
            config: McpServerConfig {
                transport: McpServerTransportConfig::Stdio {
                    command: "python".to_string(),
                    args: vec!["-m".to_string(), name.to_string()],
                    env: None,
                    env_vars: Vec::new(),
                    cwd: None,
                },
                environment_id: codex_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
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
                tools: HashMap::new(),
            },
            source,
            origin_path: PathBuf::from("/tmp/example.json"),
            fingerprint: "fp".to_string(),
        }
    }

    #[test]
    fn own_sources_are_always_approved() {
        let store = ConsentStore::with_record(PathBuf::from("/dev/null"), ConsentRecord::default());
        let server = sample_server("github", ExternalMcpSource::Own);
        assert_eq!(store.decide(&server), ConsentDecision::Approved);
    }

    #[test]
    fn external_sources_default_to_pending_then_persist() {
        let temp = TempDir::new().expect("temp dir");
        let mut store = ConsentStore::load(temp.path());
        let server = sample_server("github", ExternalMcpSource::CopilotCli);
        assert_eq!(store.decide(&server), ConsentDecision::Pending);

        store.approve("github").expect("approve");
        assert_eq!(store.decide(&server), ConsentDecision::Approved);

        let reloaded = ConsentStore::load(temp.path());
        assert_eq!(reloaded.decide(&server), ConsentDecision::Approved);
    }

    #[test]
    fn deny_then_auto_approve_keeps_deny() {
        let temp = TempDir::new().expect("temp dir");
        let mut store = ConsentStore::load(temp.path());
        let server = sample_server("github", ExternalMcpSource::CopilotCli);
        store.deny("github").expect("deny");
        store.set_auto_approve(true).expect("toggle auto");
        assert_eq!(store.decide(&server), ConsentDecision::Denied);
    }
}
