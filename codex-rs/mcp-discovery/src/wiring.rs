//! Glue layer that turns a [`DiscoveryReport`] into something the embedder
//! can merge directly into its MCP server map at agent start time.
//!
//! The wiring stays in this crate so `codex-core` does not have to grow new
//! discovery logic. Callers are expected to:
//!
//! 1. Build the user's reserved server names (typically the keys of the
//!    user's `[mcp_servers]` map plus the names contributed by active
//!    plugins) and pass them via [`ApplyDiscoveryInputs::reserved_names`].
//! 2. Provide an [`ExternalMcpEnv`] (production code can use
//!    [`crate::env::RealExternalMcpEnv`]).
//! 3. Inspect the returned [`ApplyDiscoveryOutcome`] to learn which servers
//!    were merged in, which were suppressed (`shadows`), and which still
//!    need explicit consent (`pending`).
//!
//! The wiring is intentionally headless. It never prompts and never opens
//! sockets. Pending entries are simply omitted from `merged_servers` so the
//! existing MCP connection manager will not try to launch them; the CLI/TUI
//! is responsible for translating `pending` into an interactive prompt.

use std::collections::HashMap;
use std::path::Path;

use codex_config::McpServerConfig;
use codex_config::types::ExternalMcpAutoApprove;
use codex_config::types::ExternalMcpDiscoveryToml;

use crate::consent::ConsentDecision;
use crate::consent::ConsentStore;
use crate::discover::ReservedName;
use crate::discover::ReservedNames;
use crate::discover::SelfReferenceConfig;
use crate::discover::SourceFilter;
use crate::discover::discover_with_options;
use crate::env::ExternalMcpEnv;
use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;
use crate::types::McpDiscoveryShadow;
use crate::types::ShadowReason;

/// Resolved settings for a discovery pass. Builds the per-source filter and
/// translates the user's `auto_approve` choice into the
/// [`ConsentStore`]-driven behavior the wiring layer enforces.
#[derive(Debug, Clone)]
pub struct DiscoverySettings {
    pub enabled: bool,
    pub auto_approve: ExternalMcpAutoApprove,
    pub source_filter: SourceFilter,
    pub dedupe_by_content: bool,
    /// Source labels that were rejected when parsing `sources = [...]`. The
    /// embedder should surface these as startup warnings so misconfiguration
    /// is visible.
    pub unknown_source_labels: Vec<String>,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_approve: ExternalMcpAutoApprove::Trusted,
            source_filter: SourceFilter::all(),
            dedupe_by_content: true,
            unknown_source_labels: Vec::new(),
        }
    }
}

impl DiscoverySettings {
    /// Build settings from the user's TOML section. A missing section keeps
    /// discovery disabled (`enabled: false`).
    pub fn from_toml(toml: Option<&ExternalMcpDiscoveryToml>) -> Self {
        let Some(toml) = toml else {
            return Self::default();
        };
        let enabled = toml.enabled.unwrap_or(false);
        let auto_approve = toml.auto_approve.unwrap_or_default();
        let (source_filter, unknown_source_labels) = match toml.sources.as_ref() {
            None => (SourceFilter::all(), Vec::new()),
            Some(labels) => {
                let unknown: Vec<String> = labels
                    .iter()
                    .filter(|label| ExternalMcpSource::from_label(label).is_none())
                    .cloned()
                    .collect();
                (SourceFilter::from_labels(labels.iter()), unknown)
            }
        };
        Self {
            enabled,
            auto_approve,
            source_filter,
            dedupe_by_content: toml.dedupe_by_content.unwrap_or(true),
            unknown_source_labels,
        }
    }
}

/// Inputs to [`apply_discovery`]. Borrowed from the caller so the wiring layer
/// can stay allocation-light.
pub struct ApplyDiscoveryInputs<'a> {
    pub settings: &'a DiscoverySettings,
    pub env: &'a dyn ExternalMcpEnv,
    /// Names already claimed by the user's `[mcp_servers]` map or by an
    /// active plugin. Discovered entries with one of these names become
    /// `NameCollision` shadows so the embedder can show why the external
    /// definition was ignored.
    pub reserved_names: &'a [ReservedName<'a>],
    /// Additional self-reference names. Useful when the embedder ships
    /// multiple binaries that should never be re-launched as MCP children.
    pub self_reference: &'a SelfReferenceConfig<'a>,
    /// Optional override for the consent store path. When `None` the wiring
    /// layer reads `<codex_home>/mcp-consent.json` via the supplied `env`.
    pub consent_store: Option<&'a ConsentStore>,
}

/// Output produced after merging the discovery report with the consent store.
/// `merged_servers` is what the embedder should hand to its MCP connection
/// manager; `shadows` and `pending` are diagnostic.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyDiscoveryOutcome {
    /// Discovered servers that were approved (explicitly or by source trust)
    /// and have no name collision with the embedder's existing map.
    pub merged_servers: HashMap<String, McpServerConfig>,
    /// Discovered servers that were suppressed by dedup, reserved names, or
    /// self-reference detection. Mirrors [`crate::discover::DiscoveryReport`]
    /// after consent filtering has been applied.
    pub shadows: Vec<McpDiscoveryShadow>,
    /// Discovered servers waiting on an explicit consent decision. The
    /// embedder may prompt the user or display this list as an audit trail.
    pub pending: Vec<PendingMcpServer>,
    /// Source labels that were specified in `sources = [...]` but did not
    /// match any known discovery source.
    pub unknown_source_labels: Vec<String>,
}

/// A discovered server held back because no consent decision exists yet.
/// The wire-up layer does not connect to these until the user approves them.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingMcpServer {
    pub server: DiscoveredMcpServer,
    pub decision: ConsentDecision,
}

/// Perform discovery and apply the consent gate. Returns an empty outcome
/// when discovery is disabled.
///
/// This function is synchronous and side-effect-free apart from reading the
/// consent file (when no override is supplied). It is safe to call on the
/// hot path of agent startup.
pub fn apply_discovery(inputs: ApplyDiscoveryInputs<'_>) -> ApplyDiscoveryOutcome {
    if !inputs.settings.enabled {
        return ApplyDiscoveryOutcome {
            unknown_source_labels: inputs.settings.unknown_source_labels.clone(),
            ..ApplyDiscoveryOutcome::default()
        };
    }

    let reserved = ReservedNames::from_entries(inputs.reserved_names.iter().cloned());
    let report = discover_with_options(
        inputs.env,
        &reserved,
        inputs.self_reference,
        &inputs.settings.source_filter,
        inputs.settings.dedupe_by_content,
    );

    let owned_consent;
    let consent: &ConsentStore = match inputs.consent_store {
        Some(store) => store,
        None => {
            owned_consent = load_consent(inputs.env);
            &owned_consent
        }
    };

    let mut merged_servers = HashMap::new();
    let mut pending = Vec::new();
    let mut shadows = report.shadows;

    for server in report.servers {
        match resolve_decision(&server, consent, inputs.settings.auto_approve) {
            ConsentDecision::Approved => {
                merged_servers.insert(server.name.clone(), server.config);
            }
            ConsentDecision::Denied => {
                shadows.push(McpDiscoveryShadow {
                    name: server.name.clone(),
                    source: server.source,
                    origin_path: server.origin_path.clone(),
                    reason: ShadowReason::ExplicitlyDisabled {
                        source: "user-deny".to_string(),
                    },
                });
            }
            decision @ ConsentDecision::Pending => {
                pending.push(PendingMcpServer { server, decision });
            }
        }
    }

    ApplyDiscoveryOutcome {
        merged_servers,
        shadows,
        pending,
        unknown_source_labels: inputs.settings.unknown_source_labels.clone(),
    }
}

fn resolve_decision(
    server: &DiscoveredMcpServer,
    consent: &ConsentStore,
    auto_approve: ExternalMcpAutoApprove,
) -> ConsentDecision {
    let base = consent.decide(server);
    match (auto_approve, base) {
        (ExternalMcpAutoApprove::None, ConsentDecision::Approved) => {
            // `auto_approve = "none"` downgrades trusted sources back to
            // pending so the user still has to opt in explicitly. Explicit
            // approvals recorded in the consent file are preserved.
            if server.source.trusted_by_default()
                && !consent
                    .record()
                    .approved
                    .iter()
                    .any(|name| name == &server.name)
            {
                ConsentDecision::Pending
            } else {
                ConsentDecision::Approved
            }
        }
        (ExternalMcpAutoApprove::All, ConsentDecision::Pending) => ConsentDecision::Approved,
        (_, decision) => decision,
    }
}

fn load_consent(env: &dyn ExternalMcpEnv) -> ConsentStore {
    match env.codex_home() {
        Some(home) => ConsentStore::load(&home),
        None => ConsentStore::load(Path::new("")),
    }
}
