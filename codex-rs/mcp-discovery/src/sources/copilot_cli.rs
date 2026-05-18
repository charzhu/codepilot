//! GitHub Copilot CLI user config: `~/.copilot/mcp-config.json`.

use crate::env::ExternalMcpEnv;
use crate::sources::SourceItem;
use crate::sources::common::ParsedEntry;
use crate::sources::common::load_mcp_servers_file;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

/// Scan the Copilot CLI's MCP config file.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let Some(home) = env.home_dir() else {
        return Vec::new();
    };
    let path = home.join(".copilot").join("mcp-config.json");
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
                            source: ExternalMcpSource::CopilotCli,
                            origin_path: path.clone(),
                            fingerprint,
                        })));
                    }
                    ParsedEntry::Disabled => out.push(SourceItem::Disabled {
                        name,
                        source: ExternalMcpSource::CopilotCli,
                        origin_path: path.clone(),
                    }),
                    ParsedEntry::Invalid(err) => tracing::warn!(
                        server = %name,
                        %err,
                        "invalid Copilot CLI MCP entry",
                    ),
                }
            }
        }
        Ok(None) => {}
        Err(err) => tracing::warn!(
            path = %path.display(),
            %err,
            "failed to read Copilot CLI MCP config",
        ),
    }
    out
}
