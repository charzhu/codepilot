use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use codex_league::LeagueAgentSnapshot;
use codex_league::LeagueAgentStatus;
use codex_league::LeagueRunSnapshot;
use codex_league::LeagueRunStatus;
use codex_protocol::league::render_command;
use std::collections::VecDeque;

const DEFAULT_STATUS_RETENTION: usize = 20;

#[derive(Debug, Clone)]
pub(crate) struct LeagueRunStore {
    runs: VecDeque<LeagueRunSnapshot>,
    retention: usize,
}

impl Default for LeagueRunStore {
    fn default() -> Self {
        Self {
            runs: VecDeque::new(),
            retention: DEFAULT_STATUS_RETENTION,
        }
    }
}

impl LeagueRunStore {
    pub(crate) fn with_retention(retention: usize) -> Self {
        Self {
            runs: VecDeque::new(),
            retention,
        }
    }

    pub(crate) fn upsert(&mut self, snapshot: LeagueRunSnapshot) {
        if let Some(existing) = self
            .runs
            .iter_mut()
            .find(|run| run.run_id == snapshot.run_id)
        {
            *existing = snapshot;
        } else {
            self.runs.push_front(snapshot);
        }
        while self.runs.len() > self.retention {
            self.runs.pop_back();
        }
    }

    pub(crate) fn runs(&self) -> impl Iterator<Item = &LeagueRunSnapshot> {
        self.runs.iter()
    }

    pub(crate) fn find_agent(
        &self,
        run_id: &str,
        agent_name: &str,
    ) -> Option<(LeagueRunSnapshot, LeagueAgentSnapshot)> {
        let run = self.runs.iter().find(|run| run.run_id == run_id)?;
        let agent = run.agents.iter().find(|agent| agent.name == agent_name)?;
        Some((run.clone(), agent.clone()))
    }
}

pub(crate) fn league_status_params<'a>(
    runs: impl IntoIterator<Item = &'a LeagueRunSnapshot>,
) -> SelectionViewParams {
    let mut items = Vec::new();
    for run in runs {
        items.push(run_item(run));
        for agent in &run.agents {
            items.push(agent_item(run, agent));
        }
    }
    if items.is_empty() {
        items.push(SelectionItem {
            name: "No league runs yet.".to_string(),
            description: Some(
                "Run /league <task> to start parallel external-agent advisory work.".to_string(),
            ),
            is_disabled: true,
            ..Default::default()
        });
    }

    SelectionViewParams {
        title: Some("League Status".to_string()),
        subtitle: Some("External agent CLI execution status.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

fn run_item(run: &LeagueRunSnapshot) -> SelectionItem {
    let short_id = run.run_id.chars().take(8).collect::<String>();
    SelectionItem {
        name: format!("Run {short_id} [{}]", run.mode.as_str()),
        description: Some(format!(
            "{} · {} · {}",
            run_status_label(&run.status),
            run.cwd.display(),
            truncate(&run.task, 80)
        )),
        is_disabled: true,
        search_value: Some(format!("{} {} {}", run.run_id, run.mode.as_str(), run.task)),
        ..Default::default()
    }
}

fn agent_item(run: &LeagueRunSnapshot, agent: &LeagueAgentSnapshot) -> SelectionItem {
    let detail = format!(
        "{} · {} · exit={:?} · {}",
        agent_status_label(&agent.status),
        agent.web_capability.label(),
        agent.exit_code,
        truncate(&agent.stdout_preview, 120)
    );
    let run_id = run.run_id.clone();
    let agent_name = agent.name.clone();
    SelectionItem {
        name: format!("  {}", agent.name),
        description: Some(detail),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenLeagueAgentOutput {
                run_id: run_id.clone(),
                agent_name: agent_name.clone(),
            });
        })],
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {} {}",
            run.run_id,
            agent.name,
            render_command(&agent.command),
            agent_status_label(&agent.status),
            agent.web_capability.label()
        )),
        ..Default::default()
    }
}

pub(crate) fn league_agent_output_params(
    run: &LeagueRunSnapshot,
    agent: &LeagueAgentSnapshot,
) -> SelectionViewParams {
    let short_id = run.run_id.chars().take(8).collect::<String>();
    let command = render_command(&agent.command);
    let mut items = vec![
        detail_item("Command", command),
        detail_item("Transport", agent.transport.as_str().to_string()),
        detail_item(
            "Prompt delivery",
            agent.prompt_delivery.as_str().to_string(),
        ),
        detail_item("Web/source", agent.web_capability.label().to_string()),
        detail_item("Status", agent_status_label(&agent.status).to_string()),
        detail_item("Exit code", format!("{:?}", agent.exit_code)),
        detail_item("Duration", format!("{:?} ms", agent.duration_ms)),
        detail_item("STDOUT", empty_marker(&agent.stdout_preview)),
        detail_item("STDERR", empty_marker(&agent.stderr_preview)),
    ];
    if agent.output_truncated {
        items.push(detail_item("Output", "truncated".to_string()));
    }

    SelectionViewParams {
        title: Some(format!("League Output: {}", agent.name)),
        subtitle: Some(format!(
            "{short_id} · {} · exit={:?} · duration={:?} ms",
            agent_status_label(&agent.status),
            agent.exit_code,
            agent.duration_ms
        )),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

fn detail_item(name: &str, description: String) -> SelectionItem {
    SelectionItem {
        name: name.to_string(),
        description: Some(description),
        is_disabled: true,
        ..Default::default()
    }
}

fn empty_marker(text: &str) -> String {
    if text.trim().is_empty() {
        "<empty>".to_string()
    } else {
        text.to_string()
    }
}

fn run_status_label(status: &LeagueRunStatus) -> &'static str {
    match status {
        LeagueRunStatus::Pending => "pending",
        LeagueRunStatus::Probing => "probing",
        LeagueRunStatus::Running => "running",
        LeagueRunStatus::Completed => "completed",
        LeagueRunStatus::Failed => "failed",
        LeagueRunStatus::Cancelled => "cancelled",
    }
}

fn agent_status_label(status: &LeagueAgentStatus) -> &'static str {
    match status {
        LeagueAgentStatus::Pending => "pending",
        LeagueAgentStatus::Probing => "probing",
        LeagueAgentStatus::Running => "running",
        LeagueAgentStatus::Completed => "completed",
        LeagueAgentStatus::Failed => "failed",
        LeagueAgentStatus::TimedOut => "timed out",
        LeagueAgentStatus::Cancelled => "cancelled",
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_league::LeagueAgentStatus;
    use codex_league::LeagueRunStatus;
    use codex_league::LeagueWebCapability;
    use codex_protocol::league::LeagueMode;
    use codex_protocol::league::LeaguePromptDelivery;
    use std::path::PathBuf;

    fn snapshot() -> LeagueRunSnapshot {
        LeagueRunSnapshot {
            run_id: "run-1234567890".to_string(),
            task: "inspect output".to_string(),
            mode: LeagueMode::Debug,
            status: LeagueRunStatus::Running,
            cwd: PathBuf::from("C:\\repo"),
            agents: vec![LeagueAgentSnapshot {
                name: "copilot".to_string(),
                command: vec!["copilot".to_string(), "-p".to_string()],
                transport: codex_protocol::league::LeagueAgentTransport::Cli,
                prompt_delivery: LeaguePromptDelivery::Arg,
                status: LeagueAgentStatus::Completed,
                web_capability: LeagueWebCapability::ProvidedSourcesOnly,
                exit_code: Some(0),
                duration_ms: Some(123),
                stdout_preview: "stdout body".to_string(),
                stderr_preview: "stderr body".to_string(),
                output_truncated: true,
            }],
        }
    }

    #[test]
    fn status_agent_rows_are_selectable_and_run_rows_are_disabled() {
        let run = snapshot();
        let params = league_status_params([&run]);

        assert_eq!(params.items.len(), 2);
        assert!(params.items[0].is_disabled);
        assert!(params.items[0].actions.is_empty());
        assert!(!params.items[1].is_disabled);
        assert_eq!(params.items[1].actions.len(), 1);
    }

    #[test]
    fn output_detail_includes_command_streams_and_truncation() {
        let run = snapshot();
        let agent = &run.agents[0];
        let params = league_agent_output_params(&run, agent);

        assert_eq!(params.title.as_deref(), Some("League Output: copilot"));
        let rendered = params
            .items
            .iter()
            .map(|item| {
                format!(
                    "{}={}",
                    item.name,
                    item.description.clone().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("Command=copilot -p"));
        assert!(rendered.contains("Prompt delivery=arg"));
        assert!(rendered.contains("STDOUT=stdout body"));
        assert!(rendered.contains("STDERR=stderr body"));
        assert!(rendered.contains("Output=truncated"));
    }

    #[test]
    fn run_store_finds_agent_snapshots() {
        let run = snapshot();
        let mut store = LeagueRunStore::default();
        store.upsert(run.clone());

        let (found_run, found_agent) = store
            .find_agent(&run.run_id, "copilot")
            .expect("agent should exist");
        assert_eq!(found_run.run_id, run.run_id);
        assert_eq!(found_agent.name, "copilot");
        assert!(store.find_agent(&run.run_id, "missing").is_none());
    }
}
