# codex-mcp-discovery

External MCP server discovery for Codex. This crate scans well-known config
files from other agent tools, normalizes each entry into Codex's
`McpServerConfig`, and reports collisions so the merge layer can apply
consistent precedence rules.

The crate is **headless**: it never connects to a server, never prompts the
user, and never mutates process environment. Embedders (TUI, CLI, app-server)
own the prompting/connection lifecycle.

## Source priority (highest first)

| # | Source | Path |
|---|---|---|
| 1 | `Own` | `<codex_home>/mcp-discovery/own/mcp.json` |
| 2 | `ClaudeProject` | `./.mcp.json` (walks parents) |
| 3 | `CopilotCli` | `~/.copilot/mcp-config.json` |
| 4 | `CopilotPlugin` | `~/.copilot/installed-plugins/copilot-plugins/*/.mcp.json` |
| 5 | `VsCode` | `./.vscode/mcp.json` |
| 6 | `AgencyBuiltin` | `~/.agency/agency.toml` `[mcps.builtins]` |

The user-authored `~/.codex/config.toml` and plugin-provided MCP servers are
*always* higher priority than anything in this list; the embedder passes those
names through [`ReservedNames`](src/discover.rs) so they appear as
shadow records rather than silently overriding the canonical config.

## Deduplication

- **Name collision** — the highest-priority source wins; lower-priority entries
  become `ShadowReason::NameCollision`.
- **Content duplicate** — two entries with different names but the same
  fingerprint collapse into one. The fingerprint covers `(normalize_exe,
  args, normalized_cwd)` for stdio and `normalize_url(url)` for HTTP. Env vars
  and bearer-token names are *not* part of the fingerprint.
- **Self reference** — any entry that resolves to `codex` / `codex-mcp` /
  `codex-mcp-server` (or to a localhost URL) is dropped to avoid proxy loops.
- **Explicit disable** — `{ "foo": false }` from a higher-priority source
  blocks `foo` from leaking through from any lower-priority source.

## Consent

`ConsentStore` reads and writes `<codex_home>/mcp-consent.json`. `Own` entries
are always trusted. Every other discovery is `Pending` until the embedder
calls `approve(name)` or `deny(name)` (or flips `set_auto_approve(true)`).

## How embedders wire this up

1. On agent start, build a [`RealExternalMcpEnv`].
2. Build `ReservedNames` from the keys already populated in
   `Config::mcp_servers` and the plugin-provided map.
3. Call `discover_all(&env, &reserved, &SelfReferenceConfig::default())`.
4. For each `DiscoveredMcpServer`, consult `ConsentStore::decide`. Approved
   entries are merged into the map using the same `entry().or_insert()`
   pattern as `Config::to_mcp_config`. Pending entries are surfaced to the UI;
   denied entries are dropped.
5. Shadow records can be exposed via `codex mcp list` so users can debug why a
   discovered entry was ignored.
