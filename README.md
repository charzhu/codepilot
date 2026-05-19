# Codepilot

<p align="center"><code>npm i -g @charzhu/codepilot</code></p>
<p align="center"><strong>Codepilot</strong> is a Codex CLI distribution with GitHub Copilot-oriented integrations and MCP discovery.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

Codepilot is built on top of [OpenAI Codex CLI](https://github.com/openai/codex). It keeps the local coding-agent experience from upstream Codex while adding Codepilot-specific packaging and integrations.

---

## Quickstart

Install globally with npm:

```shell
npm install -g @charzhu/codepilot
```

Then run:

```shell
codepilot
```

Use **Sign in with ChatGPT** when prompted to use Codepilot with your ChatGPT Plus, Pro, Business, Edu, or Enterprise plan. You can also use an API key through the same Codex configuration flow.

## What Codepilot adds vs upstream Codex

| Area | Upstream Codex CLI | Codepilot |
| --- | --- | --- |
| Package | `@openai/codex` | `@charzhu/codepilot` |
| Main command | `codex` | `codepilot` |
| Windows binary | `codex.exe` | `codepilot.exe` |
| GitHub Copilot workflow | General Codex CLI behavior | GitHub Copilot / Copilot CLI oriented workflow support |
| MCP startup behavior | Uses MCP servers configured directly in `config.toml` and plugins | Can discover MCP servers from other local agent configs at startup |
| MCP discovery CLI | Not part of upstream Codex | `codepilot mcp discover` and `codepilot mcp consent ...` |

Codepilot still inherits the core Codex capabilities: local agent sessions, ChatGPT sign-in, sandboxing, MCP client/server support, app-server support, non-interactive `exec`, and the Rust CLI architecture.

## External MCP discovery

Codepilot can discover MCP servers already configured for other tools and make them available to the Codepilot agent after consent.

Enable discovery in your `~/.codex/config.toml`:

```toml
[external_mcp_discovery]
enabled = true
auto_approve = "trusted"
```

Run discovery:

```shell
codepilot mcp discover
```

For machine-readable output:

```shell
codepilot mcp discover --json
```

Approve a pending discovered server:

```shell
codepilot mcp consent approve <name>
```

Then confirm the effective MCP list:

```shell
codepilot mcp list
```

Discovery currently scans these local sources:

- Codepilot-owned overrides: `<codex_home>/mcp-discovery/own/mcp.json`
- Claude project files: `.mcp.json`
- GitHub Copilot CLI config: `~/.copilot/mcp-config.json`
- Copilot plugin MCP files under `~/.copilot/installed-plugins/`
- VS Code MCP project config: `.vscode/mcp.json`
- Agency built-in MCP config: `~/.agency/agency.toml`

Deduplication is conservative: explicit Codepilot `config.toml` entries and plugin-provided MCPs win over discovered entries, duplicate command/URL fingerprints are collapsed by priority, and self-referential MCP entries are suppressed to avoid loops. External sources stay pending unless trusted by policy or explicitly approved.

## Relationship to OpenAI Codex

Codepilot tracks the upstream [OpenAI Codex](https://github.com/openai/codex) project and keeps its Apache-2.0 license. Most documentation for core Codex behavior still applies unless Codepilot-specific behavior is called out here.

Useful upstream docs:

- [Codex Documentation](https://developers.openai.com/codex)
- [Contributing](./docs/contributing.md)
- [Installing & building](./docs/install.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
