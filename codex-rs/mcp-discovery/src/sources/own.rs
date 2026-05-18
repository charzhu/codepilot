//! Discover Codex-managed overrides in `<codex_home>/mcp-discovery/own/mcp.json`.
//!
//! The "own" file follows the same `{ "mcpServers": { ... } }` schema as
//! Claude/Copilot, but its entries are trusted: the orchestrator skips the
//! consent prompt for them.

use crate::env::ExternalMcpEnv;
use crate::sources::SourceItem;
use crate::sources::common::ParsedEntry;
use crate::sources::common::load_mcp_servers_file;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

/// Discover entries from the Codex-managed overrides file. Returns an empty
/// vector if no override file exists.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let Some(codex_home) = env.codex_home() else {
        return Vec::new();
    };
    let path = codex_home
        .join("mcp-discovery")
        .join("own")
        .join("mcp.json");
    let mut out = Vec::new();
    match load_mcp_servers_file(env, &path) {
        Ok(Some(entries)) => {
            for (name, entry) in entries {
                match entry {
                    ParsedEntry::Server(boxed_config) => {
                        let config = *boxed_config;
                        let fingerprint = crate::fingerprint::fingerprint(&config);
                        out.push(SourceItem::Server(Box::new(DiscoveredMcpServer {
                            name,
                            config,
                            source: ExternalMcpSource::Own,
                            origin_path: path.clone(),
                            fingerprint,
                        })));
                    }
                    ParsedEntry::Disabled => out.push(SourceItem::Disabled {
                        name,
                        source: ExternalMcpSource::Own,
                        origin_path: path.clone(),
                    }),
                    ParsedEntry::Invalid(err) => {
                        tracing::warn!(server = %name, %err, "invalid own MCP override entry");
                    }
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "failed to read own MCP overrides");
        }
    }
    out
}
