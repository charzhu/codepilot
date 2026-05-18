//! Orchestrate per-source discovery into a single deduplicated report.
//!
//! Source priority (highest first):
//!   1. `Own`           – `<codex_home>/mcp-discovery/own/mcp.json`
//!   2. `ClaudeProject` – `./.mcp.json` (and parent directories)
//!   3. `CopilotCli`    – `~/.copilot/mcp-config.json`
//!   4. `CopilotPlugin` – `~/.copilot/installed-plugins/copilot-plugins/*/.mcp.json`
//!   5. `VsCode`        – `./.vscode/mcp.json`
//!   6. `AgencyBuiltin` – `~/.agency/agency.toml`
//!
//! Within the discovery output we also dedup by content fingerprint so two
//! entries with different names but the same effective command collapse into
//! a single connection.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::env::ExternalMcpEnv;
use crate::fingerprint::is_self_reference;
use crate::sources::SourceItem;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;
use crate::types::McpDiscoveryShadow;
use crate::types::ShadowReason;

/// The full result of a discovery pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiscoveryReport {
    /// Servers that survived dedup, ordered by source priority then name.
    pub servers: Vec<DiscoveredMcpServer>,
    /// Entries that were suppressed (collision, duplicate, self-reference).
    pub shadows: Vec<McpDiscoveryShadow>,
}

/// Reserved server names from the user-authored config / plugin layer.
/// Discovered entries with these names are recorded as shadows so the user
/// can see why their external definition was ignored.
#[derive(Debug, Default, Clone)]
pub struct ReservedNames<'a> {
    pub entries: Vec<ReservedName<'a>>,
}

#[derive(Debug, Clone)]
pub struct ReservedName<'a> {
    pub name: &'a str,
    /// Human-readable label of the higher-priority source. For Codex itself
    /// this is typically `"config.toml"` or `"plugin:<name>"`.
    pub owner: &'a str,
}

impl<'a> ReservedNames<'a> {
    pub fn from_entries<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = ReservedName<'a>>,
    {
        Self {
            entries: iter.into_iter().collect(),
        }
    }

    fn find(&self, name: &str) -> Option<&ReservedName<'a>> {
        self.entries.iter().find(|reserved| reserved.name == name)
    }
}

/// Self-reference names supplied by the embedder. Useful when the embedder
/// has its own binary that should never be re-launched as an MCP child.
#[derive(Debug, Default, Clone)]
pub struct SelfReferenceConfig<'a> {
    pub extra_names: Vec<&'a str>,
}

/// Run every enabled source and apply name + content dedup.
pub fn discover_all(
    env: &dyn ExternalMcpEnv,
    reserved: &ReservedNames<'_>,
    self_ref: &SelfReferenceConfig<'_>,
) -> DiscoveryReport {
    discover_with_sources(env, reserved, self_ref, &SourceFilter::all())
}

/// Same as [`discover_all`] but only scans the sources allowed by `filter`.
/// Use this when the embedder wants to disable individual sources at runtime
/// (for example via a config option like
/// `external_mcp_discovery.sources = ["claude", "vscode"]`).
pub fn discover_with_sources(
    env: &dyn ExternalMcpEnv,
    reserved: &ReservedNames<'_>,
    self_ref: &SelfReferenceConfig<'_>,
    filter: &SourceFilter,
) -> DiscoveryReport {
    let raw_items = collect_raw(env, filter);
    finalize(
        raw_items, reserved, self_ref, /*dedupe_by_content*/ true,
    )
}

/// Same as [`discover_with_sources`], with control over content-based dedup.
/// This is primarily used by runtime config wiring for
/// `external_mcp_discovery.dedupe_by_content`.
pub fn discover_with_options(
    env: &dyn ExternalMcpEnv,
    reserved: &ReservedNames<'_>,
    self_ref: &SelfReferenceConfig<'_>,
    filter: &SourceFilter,
    dedupe_by_content: bool,
) -> DiscoveryReport {
    let raw_items = collect_raw(env, filter);
    finalize(raw_items, reserved, self_ref, dedupe_by_content)
}

/// Selects which discovery sources [`discover_with_sources`] scans.
///
/// `SourceFilter::all()` enables every known source; the embedder can pass a
/// narrower set by calling [`SourceFilter::from_iter`] with the labels from
/// [`ExternalMcpSource::label`].
#[derive(Debug, Clone)]
pub struct SourceFilter {
    enabled: HashSet<ExternalMcpSource>,
}

impl SourceFilter {
    /// Enable every known source.
    pub fn all() -> Self {
        Self {
            enabled: HashSet::from([
                ExternalMcpSource::Own,
                ExternalMcpSource::ClaudeProject,
                ExternalMcpSource::CopilotCli,
                ExternalMcpSource::CopilotPlugin,
                ExternalMcpSource::VsCode,
                ExternalMcpSource::AgencyBuiltin,
            ]),
        }
    }

    /// Build a filter from a list of source labels (matching
    /// [`ExternalMcpSource::label`]). Unknown labels are silently dropped; the
    /// caller is responsible for surfacing them as startup warnings if it
    /// wants to.
    pub fn from_labels<I, S>(labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut enabled = HashSet::new();
        for label in labels {
            if let Some(source) = ExternalMcpSource::from_label(label.as_ref()) {
                enabled.insert(source);
            }
        }
        Self { enabled }
    }

    /// True when `source` should participate in this discovery pass.
    pub fn is_enabled(&self, source: ExternalMcpSource) -> bool {
        self.enabled.contains(&source)
    }
}

fn collect_raw(env: &dyn ExternalMcpEnv, filter: &SourceFilter) -> Vec<SourceItem> {
    let mut items = Vec::new();
    if filter.is_enabled(ExternalMcpSource::Own) {
        items.extend(crate::sources::own::discover(env));
    }
    if filter.is_enabled(ExternalMcpSource::ClaudeProject) {
        items.extend(crate::sources::claude::discover(env));
    }
    if filter.is_enabled(ExternalMcpSource::CopilotCli) {
        items.extend(crate::sources::copilot_cli::discover(env));
    }
    if filter.is_enabled(ExternalMcpSource::CopilotPlugin) {
        items.extend(crate::sources::copilot_plugins::discover(env));
    }
    if filter.is_enabled(ExternalMcpSource::VsCode) {
        items.extend(crate::sources::vscode::discover(env));
    }
    if filter.is_enabled(ExternalMcpSource::AgencyBuiltin) {
        items.extend(crate::sources::agency::discover(env));
    }
    items
}

fn finalize(
    items: Vec<SourceItem>,
    reserved: &ReservedNames<'_>,
    self_ref: &SelfReferenceConfig<'_>,
    dedupe_by_content: bool,
) -> DiscoveryReport {
    let mut accepted_by_name: HashMap<String, DiscoveredMcpServer> = HashMap::new();
    let mut name_winner: HashMap<String, ExternalMcpSource> = HashMap::new();
    let mut disabled_names: HashMap<String, (ExternalMcpSource, PathBuf)> = HashMap::new();
    let mut fingerprint_owner: HashMap<String, String> = HashMap::new();
    let mut shadows: Vec<McpDiscoveryShadow> = Vec::new();
    let mut sorted = items;
    sorted.sort_by_key(|item| std::cmp::Reverse(priority_of(item)));

    for item in sorted {
        match item {
            SourceItem::Disabled {
                name,
                source,
                origin_path,
            } => {
                disabled_names.entry(name).or_insert((source, origin_path));
            }
            SourceItem::Server(boxed_server) => {
                let server = *boxed_server;
                if let Some(reserved_entry) = reserved.find(&server.name) {
                    shadows.push(McpDiscoveryShadow {
                        name: server.name.clone(),
                        source: server.source,
                        origin_path: server.origin_path.clone(),
                        reason: ShadowReason::NameCollision {
                            winner_source: reserved_entry.owner.to_string(),
                        },
                    });
                    continue;
                }
                if let Some((blocker_source, _path)) = disabled_names.get(&server.name) {
                    shadows.push(McpDiscoveryShadow {
                        name: server.name.clone(),
                        source: server.source,
                        origin_path: server.origin_path.clone(),
                        reason: ShadowReason::ExplicitlyDisabled {
                            source: blocker_source.label().to_string(),
                        },
                    });
                    continue;
                }
                if is_self_reference(&server.config, &self_ref.extra_names) {
                    shadows.push(McpDiscoveryShadow {
                        name: server.name.clone(),
                        source: server.source,
                        origin_path: server.origin_path.clone(),
                        reason: ShadowReason::SelfReference,
                    });
                    continue;
                }
                if let Some(existing_source) = name_winner.get(&server.name) {
                    shadows.push(McpDiscoveryShadow {
                        name: server.name.clone(),
                        source: server.source,
                        origin_path: server.origin_path.clone(),
                        reason: ShadowReason::NameCollision {
                            winner_source: existing_source.label().to_string(),
                        },
                    });
                    continue;
                }
                if dedupe_by_content
                    && let Some(existing_name) = fingerprint_owner.get(&server.fingerprint)
                {
                    shadows.push(McpDiscoveryShadow {
                        name: server.name.clone(),
                        source: server.source,
                        origin_path: server.origin_path.clone(),
                        reason: ShadowReason::ContentDuplicate {
                            winner_name: existing_name.clone(),
                        },
                    });
                    continue;
                }
                if dedupe_by_content {
                    fingerprint_owner.insert(server.fingerprint.clone(), server.name.clone());
                }
                name_winner.insert(server.name.clone(), server.source);
                accepted_by_name.insert(server.name.clone(), server);
            }
        }
    }

    let mut servers: Vec<DiscoveredMcpServer> = accepted_by_name.into_values().collect();
    servers.sort_by(|a, b| match b.source.priority().cmp(&a.source.priority()) {
        std::cmp::Ordering::Equal => a.name.cmp(&b.name),
        other => other,
    });

    // Stable shadow ordering for downstream snapshots and CLI output.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    shadows.retain(|shadow| seen.insert((shadow.name.clone(), shadow_kind_key(&shadow.reason))));
    shadows.sort_by(|a, b| match a.name.cmp(&b.name) {
        std::cmp::Ordering::Equal => shadow_kind_key(&a.reason).cmp(&shadow_kind_key(&b.reason)),
        other => other,
    });

    DiscoveryReport { servers, shadows }
}

fn priority_of(item: &SourceItem) -> u8 {
    match item {
        SourceItem::Server(boxed) => boxed.source.priority(),
        SourceItem::Disabled { source, .. } => source.priority(),
    }
}

fn shadow_kind_key(reason: &ShadowReason) -> String {
    match reason {
        ShadowReason::NameCollision { winner_source } => {
            format!("name:{winner_source}")
        }
        ShadowReason::ContentDuplicate { winner_name } => {
            format!("content:{winner_name}")
        }
        ShadowReason::SelfReference => "self".to_string(),
        ShadowReason::ExplicitlyDisabled { source } => format!("disabled:{source}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use codex_config::McpServerConfig;
    use codex_config::McpServerTransportConfig;
    use pretty_assertions::assert_eq;

    use super::*;

    fn http_server(name: &str, source: ExternalMcpSource, url: &str) -> SourceItem {
        let config = McpServerConfig {
            transport: McpServerTransportConfig::StreamableHttp {
                url: url.to_string(),
                bearer_token_env_var: None,
                http_headers: None,
                env_http_headers: None,
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
            tools: HashMap::new(),
        };
        let fingerprint = crate::fingerprint::fingerprint(&config);
        SourceItem::Server(Box::new(DiscoveredMcpServer {
            name: name.to_string(),
            config,
            source,
            origin_path: PathBuf::from(format!("/tmp/{name}.json")),
            fingerprint,
        }))
    }

    fn stdio_server(name: &str, source: ExternalMcpSource, command: &str) -> SourceItem {
        let config = McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: command.to_string(),
                args: vec!["-m".to_string(), name.to_string()],
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
            tools: HashMap::new(),
        };
        let fingerprint = crate::fingerprint::fingerprint(&config);
        SourceItem::Server(Box::new(DiscoveredMcpServer {
            name: name.to_string(),
            config,
            source,
            origin_path: PathBuf::from(format!("/tmp/{name}.json")),
            fingerprint,
        }))
    }

    #[test]
    fn higher_priority_source_wins_name_collision() {
        let items = vec![
            stdio_server("github", ExternalMcpSource::CopilotCli, "python"),
            stdio_server("github", ExternalMcpSource::Own, "python"),
        ];
        let report = finalize(
            items,
            &ReservedNames::default(),
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ true,
        );
        assert_eq!(report.servers.len(), 1);
        assert_eq!(report.servers[0].source, ExternalMcpSource::Own);
        assert_eq!(report.shadows.len(), 1);
        assert_eq!(report.shadows[0].source, ExternalMcpSource::CopilotCli);
        assert_eq!(
            report.shadows[0].reason,
            ShadowReason::NameCollision {
                winner_source: ExternalMcpSource::Own.label().to_string(),
            }
        );
    }

    #[test]
    fn content_duplicate_drops_lower_priority_entry() {
        let items = vec![
            http_server("a", ExternalMcpSource::ClaudeProject, "https://api/x"),
            http_server("b", ExternalMcpSource::CopilotCli, "HTTPS://API/x/"),
        ];
        let report = finalize(
            items,
            &ReservedNames::default(),
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ true,
        );
        assert_eq!(report.servers.len(), 1);
        assert_eq!(report.servers[0].name, "a");
        assert_eq!(
            report.shadows[0].reason,
            ShadowReason::ContentDuplicate {
                winner_name: "a".to_string(),
            }
        );
    }

    #[test]
    fn content_duplicate_keeps_both_when_dedupe_disabled() {
        let items = vec![
            http_server("a", ExternalMcpSource::ClaudeProject, "https://api/x"),
            http_server("b", ExternalMcpSource::CopilotCli, "HTTPS://API/x/"),
        ];
        let report = finalize(
            items,
            &ReservedNames::default(),
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ false,
        );
        let names: Vec<&str> = report
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
        assert_eq!(report.shadows, Vec::new());
    }

    #[test]
    fn reserved_name_blocks_discovered_entry() {
        let items = vec![stdio_server(
            "github",
            ExternalMcpSource::CopilotCli,
            "python",
        )];
        let reserved = ReservedNames::from_entries([ReservedName {
            name: "github",
            owner: "config.toml",
        }]);
        let report = finalize(
            items,
            &reserved,
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ true,
        );
        assert_eq!(report.servers.len(), 0);
        assert_eq!(report.shadows.len(), 1);
        assert_eq!(
            report.shadows[0].reason,
            ShadowReason::NameCollision {
                winner_source: "config.toml".to_string(),
            }
        );
    }

    #[test]
    fn disabled_entry_blocks_lower_priority_name() {
        let items = vec![
            SourceItem::Disabled {
                name: "github".to_string(),
                source: ExternalMcpSource::Own,
                origin_path: PathBuf::from("/tmp/own.json"),
            },
            stdio_server("github", ExternalMcpSource::CopilotCli, "python"),
        ];
        let report = finalize(
            items,
            &ReservedNames::default(),
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ true,
        );
        assert_eq!(report.servers.len(), 0);
        assert_eq!(report.shadows.len(), 1);
        assert_eq!(
            report.shadows[0].reason,
            ShadowReason::ExplicitlyDisabled {
                source: ExternalMcpSource::Own.label().to_string(),
            }
        );
    }

    #[test]
    fn self_reference_is_filtered_with_extra_names() {
        let items = vec![stdio_server(
            "custom",
            ExternalMcpSource::CopilotCli,
            "codex",
        )];
        let report = finalize(
            items,
            &ReservedNames::default(),
            &SelfReferenceConfig::default(),
            /*dedupe_by_content*/ true,
        );
        assert_eq!(report.servers.len(), 0);
        assert_eq!(report.shadows[0].reason, ShadowReason::SelfReference);
    }
}
