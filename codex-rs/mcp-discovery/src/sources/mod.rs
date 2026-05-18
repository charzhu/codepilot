//! Per-source discovery modules. Each submodule scans one well-known location
//! and emits `Vec<SourceItem>`. Orchestration (priority, dedup,
//! self-reference filtering) lives in `crate::discover`.

use std::path::PathBuf;

use crate::types::DiscoveredMcpServer;
use crate::types::ExternalMcpSource;

pub(crate) mod agency;
pub(crate) mod claude;
pub(crate) mod common;
pub(crate) mod copilot_cli;
pub(crate) mod copilot_plugins;
pub(crate) mod own;
pub(crate) mod vscode;

/// Output of a single per-source scan. Disabled entries are propagated so the
/// orchestrator can suppress the same name from lower-priority sources.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SourceItem {
    Server(Box<DiscoveredMcpServer>),
    Disabled {
        name: String,
        source: ExternalMcpSource,
        origin_path: PathBuf,
    },
}
