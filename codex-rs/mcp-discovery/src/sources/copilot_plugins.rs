//! Copilot marketplace plugin discovery:
//! `~/.copilot/installed-plugins/copilot-plugins/*/.mcp.json`.

use crate::env::ExternalMcpEnv;
use crate::sources::SourceItem;
use crate::sources::common::ParsedEntry;
use crate::sources::common::load_mcp_servers_file;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

/// Enumerate each plugin directory and load its `.mcp.json` if present.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let Some(home) = env.home_dir() else {
        return Vec::new();
    };
    let plugins_dir = home
        .join(".copilot")
        .join("installed-plugins")
        .join("copilot-plugins");
    if !env.is_dir(&plugins_dir) {
        return Vec::new();
    }
    let entries = match env.read_dir(&plugins_dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(
                path = %plugins_dir.display(),
                %err,
                "failed to enumerate Copilot plugins directory",
            );
            return Vec::new();
        }
    };
    let mut sorted = entries;
    sorted.sort();
    let mut out = Vec::new();
    for plugin_dir in sorted {
        if !env.is_dir(&plugin_dir) {
            continue;
        }
        let candidate = plugin_dir.join(".mcp.json");
        match load_mcp_servers_file(env, &candidate) {
            Ok(Some(parsed)) => {
                for (name, entry) in parsed {
                    match entry {
                        ParsedEntry::Server(boxed_config) => {
                            let config = *boxed_config;
                            let fingerprint = crate::fingerprint::fingerprint(&config);
                            out.push(SourceItem::Server(Box::new(DiscoveredMcpServer {
                                name,
                                config,
                                source: ExternalMcpSource::CopilotPlugin,
                                origin_path: candidate.clone(),
                                fingerprint,
                            })));
                        }
                        ParsedEntry::Disabled => out.push(SourceItem::Disabled {
                            name,
                            source: ExternalMcpSource::CopilotPlugin,
                            origin_path: candidate.clone(),
                        }),
                        ParsedEntry::Invalid(err) => tracing::warn!(
                            server = %name,
                            path = %candidate.display(),
                            %err,
                            "invalid Copilot plugin MCP entry",
                        ),
                    }
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(
                path = %candidate.display(),
                %err,
                "failed to read Copilot plugin .mcp.json",
            ),
        }
    }
    out
}
