//! External MCP server discovery for Codex.
//!
//! Scans well-known config files from Claude Code, GitHub Copilot CLI,
//! Copilot plugins, VS Code, and Agency, normalizes each entry into a
//! [`codex_config::McpServerConfig`], and reports name/content collisions
//! so the merge layer can apply consistent precedence.
//!
//! This crate is intentionally headless: it never prompts the user or opens
//! sockets. The result is a list of discovered servers plus a consent store
//! the embedder consults before connecting.
//!
//! See the workspace-level design notes in
//! `codex-rs/mcp-discovery/README.md` for the source priority order and
//! deduplication rules.

pub use crate::consent::ConsentDecision;
pub use crate::consent::ConsentRecord;
pub use crate::consent::ConsentStore;
pub use crate::discover::DiscoveryReport;
pub use crate::discover::ReservedName;
pub use crate::discover::ReservedNames;
pub use crate::discover::SelfReferenceConfig;
pub use crate::discover::SourceFilter;
pub use crate::discover::discover_all;
pub use crate::discover::discover_with_options;
pub use crate::discover::discover_with_sources;
pub use crate::env::ExternalMcpEnv;
pub use crate::env::RealExternalMcpEnv;
pub use crate::types::DiscoveredMcpServer;
pub use crate::types::ExternalMcpSource;
pub use crate::types::McpDiscoveryShadow;
pub use crate::types::ShadowReason;
pub use crate::wiring::ApplyDiscoveryInputs;
pub use crate::wiring::ApplyDiscoveryOutcome;
pub use crate::wiring::DiscoverySettings;
pub use crate::wiring::PendingMcpServer;
pub use crate::wiring::apply_discovery;

mod consent;
mod discover;
mod env;
mod fingerprint;
mod normalize;
mod sources;
mod types;
mod wiring;
