use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::multi_agents::agent_picker_status_dot_spans;
use crate::multi_agents::format_agent_picker_item_name;
use codex_protocol::ThreadId;

pub(crate) struct FleetStatusThread {
    pub(crate) thread_id: ThreadId,
    pub(crate) agent_nickname: Option<String>,
    pub(crate) agent_role: Option<String>,
    pub(crate) is_closed: bool,
    pub(crate) is_primary: bool,
    pub(crate) is_current: bool,
}

pub(crate) fn fleet_status_params(rows: Vec<FleetStatusThread>) -> SelectionViewParams {
    let mut initial_selected_idx = None;
    let items = if rows.is_empty() {
        vec![SelectionItem {
            name: "No fleet tasks yet.".to_string(),
            description: Some("Run /fleet <task> to start a multi-agent task.".to_string()),
            is_disabled: true,
            ..Default::default()
        }]
    } else {
        rows.into_iter()
            .enumerate()
            .map(|(idx, row)| {
                if row.is_current {
                    initial_selected_idx = Some(idx);
                }
                fleet_status_item(row)
            })
            .collect()
    };

    SelectionViewParams {
        title: Some("Fleet Status".to_string()),
        subtitle: Some("Select an agent thread to inspect its transcript.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        initial_selected_idx,
        ..Default::default()
    }
}

fn fleet_status_item(row: FleetStatusThread) -> SelectionItem {
    let FleetStatusThread {
        thread_id,
        agent_nickname,
        agent_role,
        is_closed,
        is_primary,
        is_current,
    } = row;
    let name =
        format_agent_picker_item_name(agent_nickname.as_deref(), agent_role.as_deref(), is_primary);
    let uuid = thread_id.to_string();
    let status = if is_closed { "closed" } else { "running" };
    let active = if is_current { "active" } else { "inactive" };
    SelectionItem {
        name: name.clone(),
        name_prefix_spans: agent_picker_status_dot_spans(is_closed),
        description: Some(format!("{status} · {active} · {uuid}")),
        is_current,
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::SelectAgentThread(thread_id));
        })],
        dismiss_on_select: true,
        search_value: Some(format!("{name} {status} {active} {uuid}")),
        ..Default::default()
    }
}
