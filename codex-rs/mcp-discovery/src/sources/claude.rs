//! Claude Code project discovery: scan `./.mcp.json` and walk parent directories.

use std::path::PathBuf;

use crate::env::ExternalMcpEnv;
use crate::sources::SourceItem;
use crate::sources::common::ParsedEntry;
use crate::sources::common::load_mcp_servers_file;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

/// Walk from `env.cwd()` upward, collecting `.mcp.json` entries. Closer files
/// appear first in the returned list; the orchestrator's name-dedup then
/// implicitly picks the most-specific file for each server name.
pub(crate) fn discover(env: &dyn ExternalMcpEnv) -> Vec<SourceItem> {
    let mut out = Vec::new();
    let mut current: Option<PathBuf> = Some(env.cwd().to_path_buf());
    let mut visited = std::collections::HashSet::new();
    while let Some(dir) = current.take() {
        if !visited.insert(dir.clone()) {
            break;
        }
        let candidate = dir.join(".mcp.json");
        match load_mcp_servers_file(env, &candidate) {
            Ok(Some(entries)) => push_entries(&candidate, entries, &mut out),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                path = %candidate.display(),
                %err,
                "failed to read Claude project .mcp.json",
            ),
        }
        current = dir.parent().map(PathBuf::from);
    }
    out
}

fn push_entries(
    path: &std::path::Path,
    entries: std::collections::HashMap<String, ParsedEntry>,
    out: &mut Vec<SourceItem>,
) {
    for (name, entry) in entries {
        match entry {
            ParsedEntry::Server(boxed_config) => {
                let config = *boxed_config;
                let fingerprint = crate::fingerprint::fingerprint(&config);
                out.push(SourceItem::Server(Box::new(DiscoveredMcpServer {
                    name,
                    config,
                    source: ExternalMcpSource::ClaudeProject,
                    origin_path: path.to_path_buf(),
                    fingerprint,
                })));
            }
            ParsedEntry::Disabled => out.push(SourceItem::Disabled {
                name,
                source: ExternalMcpSource::ClaudeProject,
                origin_path: path.to_path_buf(),
            }),
            ParsedEntry::Invalid(err) => tracing::warn!(
                server = %name,
                path = %path.display(),
                %err,
                "invalid Claude .mcp.json entry",
            ),
        }
    }
}
