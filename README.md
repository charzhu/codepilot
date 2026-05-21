# Codepilot

<p align="center"><code>npm i -g @charzhu/codepilot</code></p>
<p align="center"><strong>Codepilot</strong> is a Codex CLI distribution with GitHub Copilot-oriented integrations, MCP discovery, multi-agent workflows, and a more customizable TUI.</p>
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>

Codepilot is built on top of [OpenAI Codex CLI](https://github.com/openai/codex). It keeps the local coding-agent experience from upstream Codex while adding Codepilot-specific packaging, GitHub Copilot integration, richer MCP behavior, and TUI personalization.

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

For GitHub Copilot-backed models and the built-in GitHub MCP server, sign in with GitHub Copilot:

```shell
codepilot login github-copilot
```

You can also start this from inside the TUI:

```text
/login github-copilot
```

## What Codepilot adds vs upstream Codex

| Area | Upstream Codex CLI | Codepilot |
| --- | --- | --- |
| Package | `@openai/codex` | `@charzhu/codepilot` |
| Main command | `codex` | `codepilot` |
| Windows binary | `codex.exe` | `codepilot.exe` |
| GitHub Copilot workflow | General Codex CLI behavior | GitHub Copilot login, models, and built-in GitHub MCP workflow support |
| GitHub MCP | User-configured MCP only | Auto-adds `github-mcp-server` when GitHub Copilot auth is available |
| MCP startup behavior | Uses MCP servers configured directly in `config.toml` and plugins | Can discover MCP servers from other local agent configs at startup |
| MCP discovery CLI | Not part of upstream Codex | `codepilot mcp discover` and `codepilot mcp consent ...` |
| Multi-agent command | Manual agent spawning | `/fleet <task>` orchestration prompt and `/fleet status` board |
| TUI personalization | Themes | Skins, terminal pets, and syntax themes |

Codepilot still inherits the core Codex capabilities: local agent sessions, ChatGPT sign-in, sandboxing, MCP client/server support, app-server support, non-interactive `exec`, and the Rust CLI architecture.

## Daily TUI commands

| Command | What it does |
| --- | --- |
| `/login github-copilot` | Sign in for GitHub Copilot models and built-in GitHub MCP auth. |
| `/model` | Choose a model, including GitHub Copilot models when signed in. |
| `/mcp` | Show configured MCP servers, tools, and auth status. |
| `/mcp verbose` | Show fuller MCP server details. |
| `/fleet <task>` | Ask Codepilot to orchestrate a multi-agent workflow for a task. |
| `/fleet status` | Open the Fleet Status board for active and completed agent threads. |
| `/tasks` | Hidden alias for `/fleet status`. |
| `/skin` | Choose a built-in TUI skin, including background and surface colors. |
| `/skin default` | Clear the skin override and use the default TUI appearance. |
| `/pets` or `/pet` | Choose, preview, or hide an ambient terminal pet. |
| `/pets disable` | Persistently disable terminal pets. |

## GitHub Copilot MCP

When GitHub Copilot auth exists in the active `CODEX_HOME`, Codepilot automatically adds a built-in MCP server named `github-mcp-server`:

```text
github-mcp-server -> https://api.enterprise.githubcopilot.com/mcp
```

The built-in server uses the same GitHub Copilot auth path as Copilot models. It exposes GitHub and Copilot tools such as `web_search`, `search_code`, `get_file_contents`, repository/issue/PR operations, Copilot Spaces tools, and secret scanning.

Check MCP startup state from the TUI:

```text
/mcp
/mcp verbose
```

Or from the command line:

```shell
codepilot mcp list
```

If `github-mcp-server` is not authenticated, sign in using the same `CODEX_HOME` as your session, then restart Codepilot:

```shell
codepilot login github-copilot
```

You can still override the built-in server by defining your own `[mcp_servers.github-mcp-server]` entry in `~/.codex/config.toml`; explicit user config takes precedence.

## Fleet mode

Fleet mode is a prompt-level orchestration command for multi-agent work. It keeps you as the root orchestrator while prompting the model to delegate independent workstreams, track status, retry narrow failures, verify important claims, and close subagents when done.

Start a fleet task in the TUI:

```text
/fleet implement retry handling for failed MCP startups
```

Open the status board:

```text
/fleet status
```

The hidden `/tasks` alias opens the same board:

```text
/tasks
```

Fleet mode is also available in non-interactive exec prompts:

```shell
codepilot exec "/fleet audit this branch and summarize risks"
```

## TUI skins

Skins are broader than syntax themes: they recolor the TUI background, transcript surfaces, text, borders, selection, and semantic accents. The `default` skin means no additional skin is applied.

Open the picker:

```text
/skin
```

Apply a skin directly:

```text
/skin neon-circuit
```

Return to the default appearance:

```text
/skin default
```

Persist a skin in `~/.codex/config.toml`:

```toml
[tui]
skin = "obsidian-bloom"
```

Built-in skin IDs:

```text
default
obsidian-bloom
porcelain-ink
deep-ocean
paper-lantern
neon-circuit
evergreen-desk
graphite-rose
solar-flare
arctic-glass
desert-night
phosphor-crt
plum-terminal
blueprint
candy-shell
monochrome-pro
```

## Terminal pets

Terminal pets are ambient sprite companions rendered inside the TUI. They can show idle/running/waiting/review/failed states while you work.

Open the picker:

```text
/pets
```

Select a built-in pet directly:

```text
/pets dewey
```

Disable pets:

```text
/pets disable
```

Persist a pet in `~/.codex/config.toml`:

```toml
[tui]
pet = "codex"
pet_anchor = "composer"       # or "screen-bottom"
```

Built-in pet IDs:

```text
codex
dewey
fireball
rocky
seedy
stacky
bsod
null-signal
```

Custom pets can live under `$CODEX_HOME/pets/<pet-id>/pet.json`. Legacy avatar directories under `$CODEX_HOME/avatars/<id>/avatar.json` are also discovered by the picker. Terminal pets require terminal image support; use a terminal with Kitty graphics, Sixel, or supported iTerm2 image rendering. Pets are disabled in tmux and Zellij because terminal images are not reliably pane-local there.

## Web search

Codepilot supports the native hosted `web_search` tool when the selected model/provider supports it and policy allows it.

Enable live web search for a TUI session:

```shell
codepilot --search
```

Or configure it explicitly:

```toml
web_search = "live"
```

The built-in GitHub Copilot MCP server also exposes an MCP `web_search` tool when GitHub Copilot auth and server startup succeed.

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
