const DEFAULT_MAX_CONCURRENT_AGENTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FleetPromptOptions {
    pub multi_agent_v2_enabled: bool,
    pub max_concurrent_agents: Option<usize>,
}

impl FleetPromptOptions {
    fn max_concurrent_agents(self) -> usize {
        self.max_concurrent_agents
            .filter(|threads| *threads > 0)
            .unwrap_or(DEFAULT_MAX_CONCURRENT_AGENTS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FleetCommandKind {
    Run,
    Status,
    List,
    Show,
    Cancel,
    NotFleet,
    UsageError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetPromptExpansion {
    pub kind: FleetCommandKind,
    pub model_text: String,
    pub history_text: String,
    pub task: String,
    pub target: Option<String>,
}

pub const FLEET_USAGE: &str =
    "Usage: /fleet <task> | /fleet status [id] | list | show [id] | cancel <run|worker>";
pub const FLEET_ORIGINAL_REQUEST_MARKER: &str = "Original user request:\n";

pub fn fleet_task_offset(model_text: &str) -> Option<usize> {
    model_text
        .find(FLEET_ORIGINAL_REQUEST_MARKER)
        .map(|offset| offset + FLEET_ORIGINAL_REQUEST_MARKER.len())
}

pub fn expand_fleet_prompt(input: &str, options: FleetPromptOptions) -> FleetPromptExpansion {
    let trimmed = input.trim();
    if trimmed.eq_ignore_ascii_case("/tasks") {
        return FleetPromptExpansion {
            kind: FleetCommandKind::Status,
            model_text: String::new(),
            history_text: "/tasks".to_string(),
            task: String::new(),
            target: None,
        };
    }

    let Some(rest) = trimmed.strip_prefix("/fleet") else {
        return FleetPromptExpansion {
            kind: FleetCommandKind::NotFleet,
            model_text: input.to_string(),
            history_text: input.to_string(),
            task: String::new(),
            target: None,
        };
    };

    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return FleetPromptExpansion {
            kind: FleetCommandKind::NotFleet,
            model_text: input.to_string(),
            history_text: input.to_string(),
            task: String::new(),
            target: None,
        };
    }

    let task = rest.trim();
    if task.is_empty() {
        return FleetPromptExpansion {
            kind: FleetCommandKind::UsageError,
            model_text: FLEET_USAGE.to_string(),
            history_text: "/fleet".to_string(),
            task: String::new(),
            target: None,
        };
    }

    let mut parts = task.splitn(2, char::is_whitespace);
    let subcommand = parts.next().unwrap_or_default();
    let target = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if subcommand.eq_ignore_ascii_case("status") {
        let history_text = target
            .map(|target| format!("/fleet status {target}"))
            .unwrap_or_else(|| "/fleet status".to_string());
        return FleetPromptExpansion {
            kind: FleetCommandKind::Status,
            model_text: String::new(),
            history_text,
            task: String::new(),
            target: target.map(str::to_string),
        };
    }
    if subcommand.eq_ignore_ascii_case("list") || subcommand.eq_ignore_ascii_case("ls") {
        return FleetPromptExpansion {
            kind: FleetCommandKind::List,
            model_text: String::new(),
            history_text: "/fleet list".to_string(),
            task: String::new(),
            target: None,
        };
    }
    if subcommand.eq_ignore_ascii_case("show") {
        let history_text = target
            .map(|target| format!("/fleet show {target}"))
            .unwrap_or_else(|| "/fleet show".to_string());
        return FleetPromptExpansion {
            kind: FleetCommandKind::Show,
            model_text: String::new(),
            history_text,
            task: String::new(),
            target: target.map(str::to_string),
        };
    }
    if subcommand.eq_ignore_ascii_case("cancel") || subcommand.eq_ignore_ascii_case("stop") {
        let Some(target) = target else {
            return FleetPromptExpansion {
                kind: FleetCommandKind::UsageError,
                model_text: "Usage: /fleet cancel <run|worker>".to_string(),
                history_text: "/fleet cancel".to_string(),
                task: String::new(),
                target: None,
            };
        };
        return FleetPromptExpansion {
            kind: FleetCommandKind::Cancel,
            model_text: String::new(),
            history_text: format!("/fleet cancel {target}"),
            task: String::new(),
            target: Some(target.to_string()),
        };
    }

    let history_text = format!("/fleet {task}");
    FleetPromptExpansion {
        kind: FleetCommandKind::Run,
        model_text: fleet_prompt(task, options),
        history_text,
        task: task.to_string(),
        target: None,
    }
}

fn fleet_prompt(task: &str, options: FleetPromptOptions) -> String {
    let max_concurrent_agents = options.max_concurrent_agents();
    let backend_guidance = if options.multi_agent_v2_enabled {
        "Backend: multi_agent_v2. Prefer named task paths and explicit task names. Use spawn_agent, send_message, followup_task, wait_agent, list_agents, and close_agent when available."
    } else {
        "Backend: stable multi-agent. Treat agent thread ids as opaque. Use stable labels in your own notes. Use spawn_agent, wait_agent, send_input, resume_agent, and close_agent when available."
    };

    format!(
        r#"<fleet_mode>
User invoked /fleet.

{FLEET_ORIGINAL_REQUEST_MARKER}{task}

Core objective:
Coordinate multiple focused agents to complete the user's request faster and with better coverage, while you remain responsible for correctness, integration, and final delivery.

Concurrency budget:
- Use at most {max_concurrent_agents} concurrent agent threads unless the user explicitly asks for more.
- Keep one thread for yourself as the orchestrator; only delegate work that can proceed independently.

Backend guidance:
- {backend_guidance}

Required workflow:
1. Restate the goal and identify independent workstreams.
2. Delegate bounded, self-contained tasks with clear expected outputs.
3. Keep the critical path local; do not wait on agents for work you can immediately do yourself.
4. Track agent status, integrate results, and resolve conflicts yourself.
5. Close agents when their results are no longer needed.

Subagent task design:
- Give each agent a precise scope, relevant files or commands, and a concrete deliverable.
- Tell coding agents they are not alone in the codebase and must not revert others' edits.
- Avoid duplicate assignments unless you intentionally want independent verification.
- Prefer read-only exploration for uncertain areas and disjoint file ownership for implementation.

Tool-use guidance:
- When a delegated task needs current, niche, external, repository, or organization-specific information, explicitly tell the subagent to use available search, web, MCP, app, or repository tools rather than relying on memory alone.
- If tools such as web_search, search_code, get_file_contents, or MCP/app tools are visible, name the relevant tools in the subagent prompt when they are appropriate for the task.
- If the needed tools are not visible, ask the subagent to use tool discovery when available and to report the limitation instead of guessing.
- When integrating results, distinguish tool-verified claims from memory-derived or unverified claims.

Root responsibilities:
- You own the final plan, code integration, validation, and user-facing response.
- Verify important agent claims before relying on them when cheap or safety-critical.
- Keep changes minimal, avoid unrelated refactors, and preserve existing behavior.

Retry policy:
- If an agent fails because the task was ambiguous or under-specified, retry once with tighter instructions.
- If an agent is blocked by missing permissions, unavailable tools, or repeated failures, continue locally and report the limitation.
- Do not spin endlessly; prefer partial progress with clear status over hidden retries.

Integration rules:
- Review all returned changes before treating them as accepted.
- Resolve overlapping edits conservatively and keep the smallest coherent patch.
- Run the narrowest relevant validation first, then broader checks if warranted.

Progress reporting:
- Provide concise status updates when launching agents, when results arrive, and when switching phases.
- Keep the user informed without exposing noisy implementation details.

Final response:
- Summarize what changed, how it was validated, and any remaining risks or next steps.
</fleet_mode>"#
    )
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn options() -> FleetPromptOptions {
        FleetPromptOptions {
            multi_agent_v2_enabled: false,
            max_concurrent_agents: Some(3),
        }
    }

    #[test]
    fn expands_fleet_run_prompt() {
        let expansion = expand_fleet_prompt("/fleet build the feature", options());

        assert_eq!(expansion.kind, FleetCommandKind::Run);
        assert_eq!(expansion.history_text, "/fleet build the feature");
        assert_eq!(expansion.task, "build the feature");
        assert!(expansion.model_text.contains("<fleet_mode>"));
        assert!(
            expansion
                .model_text
                .contains("Original user request:\nbuild the feature")
        );
        assert!(
            expansion
                .model_text
                .contains("at most 3 concurrent agent threads")
        );
        assert!(expansion.model_text.contains("Retry policy:"));
        assert!(expansion.model_text.contains("stable multi-agent"));
        assert!(expansion.model_text.contains("Tool-use guidance:"));
        assert!(expansion.model_text.contains("web_search"));
        assert!(
            expansion
                .model_text
                .contains("distinguish tool-verified claims")
        );
        let task_offset = fleet_task_offset(&expansion.model_text).expect("task offset");
        assert_eq!(
            &expansion.model_text[task_offset..][.."build the feature".len()],
            "build the feature"
        );
    }

    #[test]
    fn expands_fleet_v2_guidance() {
        let expansion = expand_fleet_prompt(
            "/fleet validate shards",
            FleetPromptOptions {
                multi_agent_v2_enabled: true,
                max_concurrent_agents: None,
            },
        );

        assert_eq!(expansion.kind, FleetCommandKind::Run);
        assert!(expansion.model_text.contains("multi_agent_v2"));
        assert!(
            expansion
                .model_text
                .contains("at most 4 concurrent agent threads")
        );
    }

    #[test]
    fn parses_status_and_usage() {
        assert_eq!(
            expand_fleet_prompt("/fleet status", options()).kind,
            FleetCommandKind::Status
        );
        let status_with_id = expand_fleet_prompt("/fleet status abc", options());
        assert_eq!(status_with_id.kind, FleetCommandKind::Status);
        assert_eq!(status_with_id.target.as_deref(), Some("abc"));
        assert_eq!(
            expand_fleet_prompt("/fleet list", options()).kind,
            FleetCommandKind::List
        );
        assert_eq!(
            expand_fleet_prompt("/fleet show abc", options()).kind,
            FleetCommandKind::Show
        );
        assert_eq!(
            expand_fleet_prompt("/fleet cancel abc", options()).kind,
            FleetCommandKind::Cancel
        );
        assert_eq!(
            expand_fleet_prompt("/tasks", options()).kind,
            FleetCommandKind::Status
        );
        assert_eq!(
            expand_fleet_prompt("/fleet", options()).kind,
            FleetCommandKind::UsageError
        );
    }

    #[test]
    fn ignores_non_fleet_prefixes() {
        assert_eq!(
            expand_fleet_prompt("/fleetness test", options()).kind,
            FleetCommandKind::NotFleet
        );
        assert_eq!(
            expand_fleet_prompt("hello", options()).kind,
            FleetCommandKind::NotFleet
        );
    }
}
