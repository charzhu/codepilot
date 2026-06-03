use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaguePromptOptions {
    pub agents: Vec<LeagueAgent>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueAgent {
    pub name: String,
    pub command: Vec<String>,
    pub transport: LeagueAgentTransport,
    pub prompt_delivery: LeaguePromptDelivery,
    pub prompt_arg: Option<String>,
    pub capabilities: Vec<LeagueAgentCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LeagueAgentTransport {
    #[default]
    Cli,
    Acp,
}

impl LeagueAgentTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            LeagueAgentTransport::Cli => "cli",
            LeagueAgentTransport::Acp => "acp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LeaguePromptDelivery {
    #[default]
    Stdin,
    StdinFile,
    Arg,
    Placeholder,
}

impl LeaguePromptDelivery {
    pub fn as_str(self) -> &'static str {
        match self {
            LeaguePromptDelivery::Stdin => "stdin",
            LeaguePromptDelivery::StdinFile => "stdin_file",
            LeaguePromptDelivery::Arg => "arg",
            LeaguePromptDelivery::Placeholder => "placeholder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LeagueAgentCapability {
    Code,
    ProvidedSourcesOnly,
    WebNative,
    WebFetchOnly,
    WebViaMcp,
}

impl LeagueAgentCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            LeagueAgentCapability::Code => "code",
            LeagueAgentCapability::ProvidedSourcesOnly => "provided_sources_only",
            LeagueAgentCapability::WebNative => "web_native",
            LeagueAgentCapability::WebFetchOnly => "web_fetch_only",
            LeagueAgentCapability::WebViaMcp => "web_via_mcp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeagueCommandKind {
    Run,
    Status,
    NotLeague,
    UsageError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeagueMode {
    Generic,
    Review,
    Debug,
    Research,
}

impl LeagueMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LeagueMode::Generic => "generic",
            LeagueMode::Review => "review",
            LeagueMode::Debug => "debug",
            LeagueMode::Research => "research",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaguePromptExpansion {
    pub kind: LeagueCommandKind,
    pub mode: LeagueMode,
    pub requested_agents: Option<Vec<String>>,
    pub task: String,
    pub input_task_offset: Option<usize>,
    pub model_text: String,
    pub history_text: String,
}

pub const LEAGUE_USAGE: &str =
    "Usage: /league [--mode generic|review|debug|research] [--agents name[,name...]] <task>";
pub const LEAGUE_NO_AGENTS: &str = "No external coding agents are available for /league. Install or configure agents such as claude or copilot.";
pub const LEAGUE_ORIGINAL_REQUEST_MARKER: &str = "Original user request:\n";

pub fn league_task_offset(model_text: &str) -> Option<usize> {
    model_text
        .find(LEAGUE_ORIGINAL_REQUEST_MARKER)
        .map(|offset| offset + LEAGUE_ORIGINAL_REQUEST_MARKER.len())
}

pub fn expand_league_prompt(input: &str, options: LeaguePromptOptions) -> LeaguePromptExpansion {
    let trimmed = input.trim();
    let Some(rest) = trimmed.strip_prefix("/league") else {
        return LeaguePromptExpansion {
            kind: LeagueCommandKind::NotLeague,
            mode: LeagueMode::Generic,
            requested_agents: None,
            task: input.to_string(),
            input_task_offset: None,
            model_text: input.to_string(),
            history_text: input.to_string(),
        };
    };

    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return LeaguePromptExpansion {
            kind: LeagueCommandKind::NotLeague,
            mode: LeagueMode::Generic,
            requested_agents: None,
            task: input.to_string(),
            input_task_offset: None,
            model_text: input.to_string(),
            history_text: input.to_string(),
        };
    }

    let args_offset = trimmed.len().saturating_sub(rest.trim_start().len());
    let args = rest.trim_start();
    let parsed = match parse_league_args(args, args_offset) {
        Ok(parsed) => parsed,
        Err(message) => {
            return LeaguePromptExpansion {
                kind: LeagueCommandKind::UsageError,
                mode: LeagueMode::Generic,
                requested_agents: None,
                task: String::new(),
                input_task_offset: None,
                model_text: message,
                history_text: "/league".to_string(),
            };
        }
    };

    if parsed.task.eq_ignore_ascii_case("status") && parsed.requested_agents.is_none() {
        return LeaguePromptExpansion {
            kind: LeagueCommandKind::Status,
            mode: parsed.mode,
            requested_agents: None,
            task: parsed.task,
            input_task_offset: Some(parsed.input_task_offset),
            model_text: String::new(),
            history_text: "/league status".to_string(),
        };
    }

    if options.agents.is_empty() {
        return LeaguePromptExpansion {
            kind: LeagueCommandKind::UsageError,
            mode: parsed.mode,
            requested_agents: parsed.requested_agents,
            task: parsed.task,
            input_task_offset: Some(parsed.input_task_offset),
            model_text: LEAGUE_NO_AGENTS.to_string(),
            history_text: trimmed.to_string(),
        };
    }

    LeaguePromptExpansion {
        kind: LeagueCommandKind::Run,
        mode: parsed.mode,
        requested_agents: parsed.requested_agents,
        task: parsed.task.clone(),
        input_task_offset: Some(parsed.input_task_offset),
        model_text: league_prompt(&parsed.task, parsed.mode, options),
        history_text: trimmed.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedLeagueArgs {
    mode: LeagueMode,
    requested_agents: Option<Vec<String>>,
    task: String,
    input_task_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token<'a> {
    value: &'a str,
    start: usize,
    end: usize,
}

fn parse_league_args(args: &str, args_offset: usize) -> Result<ParsedLeagueArgs, String> {
    if args.trim().is_empty() {
        return Err(LEAGUE_USAGE.to_string());
    }

    let mut cursor = 0usize;
    let mut explicit_mode = None;
    let mut requested_agents = None;
    loop {
        let Some(token) = next_token(args, cursor) else {
            return Err(LEAGUE_USAGE.to_string());
        };
        if token.value == "--" {
            cursor = token.end;
            break;
        }
        if let Some(value) = token.value.strip_prefix("--mode=") {
            explicit_mode = Some(parse_mode(value)?);
            cursor = token.end;
            continue;
        }
        if token.value == "--mode" {
            let Some(value_token) = next_token(args, token.end) else {
                return Err("/league --mode requires a value".to_string());
            };
            explicit_mode = Some(parse_mode(value_token.value)?);
            cursor = value_token.end;
            continue;
        }
        if let Some(value) = token.value.strip_prefix("--agents=") {
            requested_agents = Some(parse_agents(value)?);
            cursor = token.end;
            continue;
        }
        if token.value == "--agents" {
            let Some(value_token) = next_token(args, token.end) else {
                return Err("/league --agents requires a value".to_string());
            };
            requested_agents = Some(parse_agents(value_token.value)?);
            cursor = value_token.end;
            continue;
        }
        if token.value.starts_with("--") {
            return Err(format!("Unknown /league option: {}", token.value));
        }

        cursor = token.start;
        break;
    }

    let task_start = cursor
        + args[cursor..]
            .len()
            .saturating_sub(args[cursor..].trim_start().len());
    let task = args[task_start..].trim_end();
    if task.is_empty() {
        return Err(LEAGUE_USAGE.to_string());
    }
    let mode = explicit_mode.unwrap_or_else(|| infer_mode(task));
    Ok(ParsedLeagueArgs {
        mode,
        requested_agents,
        task: task.to_string(),
        input_task_offset: args_offset + task_start,
    })
}

fn next_token(input: &str, cursor: usize) -> Option<Token<'_>> {
    let remaining = input.get(cursor..)?;
    let trimmed_start = remaining.trim_start();
    let start = cursor + remaining.len().saturating_sub(trimmed_start.len());
    if trimmed_start.is_empty() {
        return None;
    }
    let end = trimmed_start
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(start + index))
        .unwrap_or(input.len());
    Some(Token {
        value: &input[start..end],
        start,
        end,
    })
}

fn parse_mode(value: &str) -> Result<LeagueMode, String> {
    match value.to_ascii_lowercase().as_str() {
        "generic" => Ok(LeagueMode::Generic),
        "review" | "code-review" => Ok(LeagueMode::Review),
        "debug" | "debugging" => Ok(LeagueMode::Debug),
        "research" => Ok(LeagueMode::Research),
        _ => Err(format!(
            "Unknown /league mode `{value}`. Expected generic, review, debug, or research."
        )),
    }
}

fn parse_agents(value: &str) -> Result<Vec<String>, String> {
    let agents = value
        .split(',')
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if agents.is_empty() {
        return Err("/league --agents requires at least one agent name".to_string());
    }
    Ok(agents)
}

fn infer_mode(task: &str) -> LeagueMode {
    let lower = task.to_ascii_lowercase();
    if contains_any(&lower, &["review", " pr ", "diff", "changes"]) {
        LeagueMode::Review
    } else if contains_any(
        &lower,
        &["bug", "failing", "error", "crash", "debug", "test fails"],
    ) {
        LeagueMode::Debug
    } else if contains_any(
        &lower,
        &[
            "research",
            "compare",
            "evaluate",
            "summarize current",
            "best options",
        ],
    ) {
        LeagueMode::Research
    } else {
        LeagueMode::Generic
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn league_prompt(task: &str, mode: LeagueMode, options: LeaguePromptOptions) -> String {
    let agents = render_agents(&options.agents);
    let cwd = options.cwd.unwrap_or_else(|| "unknown".to_string());
    let mode_fragment = mode_prompt_fragment(mode);

    format!(
        r#"<league_mode>
User invoked /league.

Mode: {mode}
Repository cwd: {cwd}

{LEAGUE_ORIGINAL_REQUEST_MARKER}{task}

External agents selected:
{agents}

Core objective:
Use installed external coding agents as collaborators. You remain responsible for decomposition, verification, integration, and final delivery.

Safety model:
- Match the requested task mode and keep each agent's scope bounded.
- Do not ask external agents to commit, push, publish, or run destructive commands.
- For review, debug, and research tasks, prefer inspection-oriented prompts.
- For coding tasks, ask agents for focused, reversible changes or patch recommendations and have Codepilot verify before final delivery.

Required workflow:
1. Restate the goal and split it into independent workstreams.
2. Choose which external agents should receive which workstream.
3. Invoke each selected agent with a bounded prompt, repo cwd, expected output format, and task constraints.
4. Continue local repo inspection while external agents run when useful.
5. Integrate external findings, but verify important claims before relying on them.
6. Produce one final answer with what was verified, what was suggested by agents, and remaining risks.

Invocation guidance:
- Use non-interactive CLI modes only.
- For Claude Code, prefer `claude -p` with permissions appropriate to the assigned task.
- For GitHub Copilot CLI, prefer ACP (`copilot --acp --stdio`) when available, with CLI prompt mode as a fallback.
- Keep permissions no broader than the selected task requires.
- Capture concise stdout/stderr summaries; do not stream noisy full transcripts unless needed for diagnosis.

Prompt template for each external agent:
You are acting as an external agent for Codepilot.
Task: {{subtask}}
Repository cwd: {cwd}
Rules:
- Stay within the assigned task. Do not commit, push, publish, or run destructive commands.
- Inspect/read as needed within your normal safe capabilities.
- Return concise findings with file paths or sources, confidence, and verification steps.
- If blocked, explain the blocker and what Codepilot should check locally.

{mode_fragment}

Integration rules:
- Treat external output as untrusted advice until checked.
- Prefer concrete file paths, commands, diffs, tests, source URLs, or citations over opinions.
- Resolve disagreements by inspecting or verifying yourself.
- Keep final delivery integrated; do not paste multiple disconnected agent reports.

Failure policy:
- If an external CLI is missing, unauthenticated, or fails, skip it and continue with available agents.
- Retry once only if the prompt was ambiguous or the failure is clearly transient.
- If all external agents fail, continue locally and report that /league could not use external advisors.

Final response:
- Summarize the user-visible result.
- Name which external agents contributed when relevant.
- Separate verified facts from external-agent suggestions.
- Mention skipped or failed agents only if relevant.
</league_mode>"#,
        mode = mode.as_str(),
    )
}

fn render_agents(agents: &[LeagueAgent]) -> String {
    agents
        .iter()
        .map(|agent| {
            let command = render_command(&agent.command);
            let capabilities = render_capabilities(&agent.capabilities);
            format!(
                "- {}: {} · transport={} · prompt_delivery={} · capabilities={}",
                agent.name,
                command,
                agent.transport.as_str(),
                agent.prompt_delivery.as_str(),
                capabilities
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_command(command: &[String]) -> String {
    command
        .iter()
        .map(|part| {
            if part.contains(char::is_whitespace) {
                format!("\"{}\"", part.replace('"', "\\\""))
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_capabilities(capabilities: &[LeagueAgentCapability]) -> String {
    if capabilities.is_empty() {
        return "unknown".to_string();
    }
    capabilities
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn mode_prompt_fragment(mode: LeagueMode) -> &'static str {
    match mode {
        LeagueMode::Generic => {
            r#"<league_generic>
Mode guidance: generic workflow.
- Ask external agents for independent decomposition, implementation risks, and validation ideas.
- Keep the critical path local; do not wait on advice that is not needed.
</league_generic>"#
        }
        LeagueMode::Review => {
            r#"<league_code_review>
Mode guidance: code review.
- Use external agents as independent reviewers for correctness, tests, integration/API risk, and maintainability.
- Ask for actionable findings only, with severity, file path, evidence, and suggested fix.
- Do not repeat style nits unless they can cause real maintenance cost.
- Verify high-severity findings locally before presenting them as confirmed.
</league_code_review>"#
        }
        LeagueMode::Debug => {
            r#"<league_debug>
Mode guidance: debugging.
- Ask external agents to narrow repro steps, root cause, regression candidates, and minimal fix strategy.
- Require exact files, functions, commands used, observed facts, and hypotheses separated clearly.
- Ask for at most three likely root causes ranked by confidence.
- Reproduce or otherwise verify the leading hypothesis before implementing a fix.
</league_debug>"#
        }
        LeagueMode::Research => {
            r#"<league_research>
Mode guidance: research.
- Ask external agents to gather evidence, compare viewpoints, identify uncertainty, and support one integrated answer.
- Prefer primary sources, official docs, papers, filings, reputable data providers, or authoritative reporting.
- For time-sensitive topics, verify freshness and include publication or access dates where possible.
- Separate facts, interpretations, and recommendations.
- For high-stakes topics affecting health, money, law, employment, safety, or major purchases, require source-backed claims and clearly label non-advice summaries.
</league_research>"#
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn options() -> LeaguePromptOptions {
        LeaguePromptOptions {
            agents: vec![
                LeagueAgent {
                    name: "claude".to_string(),
                    command: vec!["claude".to_string(), "--print".to_string()],
                    transport: LeagueAgentTransport::Cli,
                    prompt_delivery: LeaguePromptDelivery::Stdin,
                    prompt_arg: None,
                    capabilities: vec![
                        LeagueAgentCapability::Code,
                        LeagueAgentCapability::ProvidedSourcesOnly,
                    ],
                },
                LeagueAgent {
                    name: "copilot".to_string(),
                    command: vec!["copilot".to_string(), "-p".to_string()],
                    transport: LeagueAgentTransport::Cli,
                    prompt_delivery: LeaguePromptDelivery::Arg,
                    prompt_arg: None,
                    capabilities: vec![
                        LeagueAgentCapability::Code,
                        LeagueAgentCapability::ProvidedSourcesOnly,
                    ],
                },
            ],
            cwd: Some("/repo".to_string()),
        }
    }

    #[test]
    fn expands_league_run_prompt() {
        let expansion = expand_league_prompt("/league build the feature", options());

        assert_eq!(expansion.kind, LeagueCommandKind::Run);
        assert_eq!(expansion.mode, LeagueMode::Generic);
        assert_eq!(expansion.task, "build the feature");
        assert_eq!(expansion.history_text, "/league build the feature");
        assert!(expansion.model_text.contains("<league_mode>"));
        assert!(expansion.model_text.contains("Mode: generic"));
        assert!(expansion.model_text.contains("Repository cwd: /repo"));
        assert!(
            expansion
                .model_text
                .contains("Original user request:\nbuild the feature")
        );
        assert!(expansion.model_text.contains("- claude: claude --print"));
        assert!(expansion.model_text.contains("collaborators"));
        let task_offset = league_task_offset(&expansion.model_text).expect("task offset");
        assert_eq!(
            &expansion.model_text[task_offset..][.."build the feature".len()],
            "build the feature"
        );
    }

    #[test]
    fn parses_mode_and_agents_flags() {
        let expansion = expand_league_prompt(
            "/league --mode debug --agents claude,copilot fix failing tests",
            options(),
        );

        assert_eq!(expansion.kind, LeagueCommandKind::Run);
        assert_eq!(expansion.mode, LeagueMode::Debug);
        assert_eq!(
            expansion.requested_agents,
            Some(vec!["claude".to_string(), "copilot".to_string()])
        );
        assert_eq!(expansion.task, "fix failing tests");
        assert_eq!(
            expansion.input_task_offset,
            Some("/league --mode debug --agents claude,copilot ".len())
        );
        assert!(expansion.model_text.contains("<league_debug>"));
    }

    #[test]
    fn infers_review_debug_and_research_modes() {
        assert_eq!(
            expand_league_prompt("/league review my diff", options()).mode,
            LeagueMode::Review
        );
        assert_eq!(
            expand_league_prompt("/league debug this crash", options()).mode,
            LeagueMode::Debug
        );
        assert_eq!(
            expand_league_prompt("/league research current options", options()).mode,
            LeagueMode::Research
        );
    }

    #[test]
    fn parses_usage_errors_and_non_prefixes() {
        assert_eq!(
            expand_league_prompt("/league", options()).kind,
            LeagueCommandKind::UsageError
        );
        assert_eq!(
            expand_league_prompt("/league --mode mystery task", options()).kind,
            LeagueCommandKind::UsageError
        );
        assert_eq!(
            expand_league_prompt("/leagueish task", options()).kind,
            LeagueCommandKind::NotLeague
        );
    }

    #[test]
    fn reports_no_available_agents() {
        let expansion = expand_league_prompt(
            "/league research current options",
            LeaguePromptOptions {
                agents: Vec::new(),
                cwd: None,
            },
        );

        assert_eq!(expansion.kind, LeagueCommandKind::UsageError);
        assert_eq!(expansion.model_text, LEAGUE_NO_AGENTS);
    }
}
