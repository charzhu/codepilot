//! Core types describing discovered external MCP servers and the bookkeeping
//! needed to merge them with the user-authored config.

use std::path::PathBuf;

use codex_config::McpServerConfig;

/// Where a discovered MCP server came from. Variants are ordered by priority:
/// earlier variants win during name-based dedup.
///
/// `Own` represents Codex-managed overrides (`<codex_home>/mcp-discovery/own`)
/// and behaves like trusted input. Everything else is treated as external and
/// requires user consent before connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExternalMcpSource {
    /// `<codex_home>/mcp-discovery/own/mcp.json` (Codex-managed overrides).
    Own,
    /// `./.mcp.json` walking up parent directories (Claude Code).
    ClaudeProject,
    /// `~/.copilot/mcp-config.json` (GitHub Copilot CLI).
    CopilotCli,
    /// `~/.copilot/installed-plugins/copilot-plugins/*/.mcp.json`.
    CopilotPlugin,
    /// `./.vscode/mcp.json` (VS Code project-local).
    VsCode,
    /// `~/.agency/agency.toml` `[mcps.builtins]` invoked via `agency mcp ...`.
    AgencyBuiltin,
}

impl ExternalMcpSource {
    /// Human-readable label used in status output. Stable; do not change without
    /// updating snapshots and docs.
    pub const fn label(self) -> &'static str {
        match self {
            ExternalMcpSource::Own => "own",
            ExternalMcpSource::ClaudeProject => "claude",
            ExternalMcpSource::CopilotCli => "copilot-cli",
            ExternalMcpSource::CopilotPlugin => "copilot-plugin",
            ExternalMcpSource::VsCode => "vscode",
            ExternalMcpSource::AgencyBuiltin => "agency",
        }
    }

    /// Parse a label (matching [`Self::label`]) back into the source enum.
    /// Returns `None` for unknown labels. Callers should treat unknown labels
    /// as a configuration mistake and surface them as a startup warning.
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "own" => Some(ExternalMcpSource::Own),
            "claude" => Some(ExternalMcpSource::ClaudeProject),
            "copilot_cli" | "copilot-cli" => Some(ExternalMcpSource::CopilotCli),
            "copilot_plugins" | "copilot-plugin" => Some(ExternalMcpSource::CopilotPlugin),
            "vscode" => Some(ExternalMcpSource::VsCode),
            "agency" => Some(ExternalMcpSource::AgencyBuiltin),
            _ => None,
        }
    }

    /// Higher number wins when two sources define the same server name.
    /// The ordering matches the lib-level documentation.
    pub const fn priority(self) -> u8 {
        match self {
            ExternalMcpSource::Own => 60,
            ExternalMcpSource::ClaudeProject => 50,
            ExternalMcpSource::CopilotCli => 40,
            ExternalMcpSource::CopilotPlugin => 30,
            ExternalMcpSource::VsCode => 20,
            ExternalMcpSource::AgencyBuiltin => 10,
        }
    }

    /// `Own` is the only source treated as trusted (no consent prompt).
    pub const fn trusted_by_default(self) -> bool {
        matches!(self, ExternalMcpSource::Own)
    }
}

/// A single discovered MCP server, ready to be merged into the main MCP map.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredMcpServer {
    /// The name used in `mcp_servers` map keys (e.g. `github`, `m365-copilot`).
    pub name: String,
    /// Normalized config in the same shape Codex consumes natively.
    pub config: McpServerConfig,
    /// Which on-disk source produced this entry.
    pub source: ExternalMcpSource,
    /// Absolute path of the file (or directory) we read this entry from. Used
    /// for status output and shadow records.
    pub origin_path: PathBuf,
    /// Cached fingerprint computed in `crate::fingerprint`. Stable for the
    /// lifetime of the discovery run.
    pub fingerprint: String,
}

/// Why a discovered entry was suppressed during dedup. Useful for status
/// reporting and unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowReason {
    /// Suppressed because the name already exists in the merged map (from a
    /// higher-priority source, the user's TOML config, or a plugin).
    NameCollision { winner_source: String },
    /// Suppressed because another entry has the same content fingerprint
    /// (different name but same `command`/`args`/`cwd` or `url`).
    ContentDuplicate { winner_name: String },
    /// Skipped because the resolved command points back at Codex itself.
    SelfReference,
    /// Suppressed because a higher-priority source set the entry to `false`.
    ExplicitlyDisabled { source: String },
}

/// Record of a discovered entry that did NOT make it into the merged map.
/// Surfaced for status output so users can see why an entry was ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct McpDiscoveryShadow {
    pub name: String,
    pub source: ExternalMcpSource,
    pub origin_path: PathBuf,
    pub reason: ShadowReason,
}
