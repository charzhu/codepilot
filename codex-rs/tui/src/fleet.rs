use crate::app_event::AppEvent;
use crate::app_server_session::AppServerFleetWorkerTurnResult;
use crate::app_server_session::ThreadParamsMode;
use crate::app_server_session::run_read_only_fleet_worker_turn;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::legacy_core::config::Config;
use codex_app_server_client::AppServerRequestHandle;
use ratatui::style::Stylize;
use ratatui::text::Line;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DEFAULT_STATUS_RETENTION: usize = 20;
const DEFAULT_MAX_CONCURRENCY: usize = 4;
const HARD_MAX_WORKERS: usize = 32;
const PLANNER_TIMEOUT: Duration = Duration::from_secs(90);
const WORKER_TIMEOUT: Duration = Duration::from_secs(600);
const DEPENDENCY_OUTPUT_LIMIT_CHARS: usize = 12_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FleetRunRequest {
    pub(crate) run_id: String,
    pub(crate) task: String,
    pub(crate) max_concurrency: usize,
    pub(crate) associated_goal_objective: Option<String>,
    pub(crate) persistence_dir: Option<PathBuf>,
    pub(crate) local_image_paths: Vec<PathBuf>,
    pub(crate) remote_image_urls: Vec<String>,
    pub(crate) mention_context: Vec<String>,
}

impl FleetRunRequest {
    pub(crate) fn new(task: String, max_concurrency: Option<usize>) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            task,
            max_concurrency: max_concurrency
                .filter(|threads| *threads > 0)
                .unwrap_or(DEFAULT_MAX_CONCURRENCY),
            associated_goal_objective: None,
            persistence_dir: None,
            local_image_paths: Vec::new(),
            remote_image_urls: Vec::new(),
            mention_context: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FleetRunStatus {
    Planning,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum FleetWorkerStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum FleetWorkerRole {
    Researcher,
    Implementer,
    Verifier,
    Critic,
    Synthesizer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FleetWorkerSnapshot {
    pub(crate) id: String,
    pub(crate) role: FleetWorkerRole,
    pub(crate) goal: String,
    pub(crate) depends_on: Vec<String>,
    pub(crate) status: FleetWorkerStatus,
    pub(crate) summary: String,
    pub(crate) details: String,
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FleetRunSnapshot {
    pub(crate) run_id: String,
    pub(crate) task: String,
    pub(crate) status: FleetRunStatus,
    pub(crate) max_concurrency: usize,
    pub(crate) associated_goal_objective: Option<String>,
    pub(crate) started_at_ms: u128,
    pub(crate) finished_at_ms: Option<u128>,
    pub(crate) workers: Vec<FleetWorkerSnapshot>,
    pub(crate) final_summary: String,
    pub(crate) planner_notes: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FleetRunStore {
    runs: VecDeque<FleetRunSnapshot>,
    retention: usize,
}

impl Default for FleetRunStore {
    fn default() -> Self {
        Self::with_retention(DEFAULT_STATUS_RETENTION)
    }
}

impl FleetRunStore {
    pub(crate) fn with_retention(retention: usize) -> Self {
        Self {
            runs: VecDeque::new(),
            retention,
        }
    }

    pub(crate) fn load_from_dir(dir: Option<PathBuf>, retention: usize) -> Self {
        let mut store = Self::with_retention(retention);
        let Some(dir) = dir else {
            return store;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return store;
        };
        let mut snapshots = entries
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .filter_map(|content| serde_json::from_str::<FleetRunSnapshot>(&content).ok())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| {
            std::cmp::Reverse(snapshot.finished_at_ms.unwrap_or(snapshot.started_at_ms))
        });
        for snapshot in snapshots.into_iter().take(retention) {
            store.upsert(snapshot);
        }
        store
    }

    pub(crate) fn upsert(&mut self, snapshot: FleetRunSnapshot) {
        if let Some(existing) = self
            .runs
            .iter_mut()
            .find(|run| run.run_id == snapshot.run_id)
        {
            if existing.status == FleetRunStatus::Cancelled
                && snapshot.status != FleetRunStatus::Cancelled
            {
                return;
            }
            *existing = snapshot;
        } else {
            self.runs.push_front(snapshot);
        }
        while self.runs.len() > self.retention {
            self.runs.pop_back();
        }
    }

    pub(crate) fn runs(&self) -> impl Iterator<Item = &FleetRunSnapshot> {
        self.runs.iter()
    }

    pub(crate) fn find(&self, run_id: &str) -> Option<FleetRunSnapshot> {
        self.runs
            .iter()
            .find(|run| short_or_full_id_matches(&run.run_id, run_id))
            .cloned()
    }

    pub(crate) fn is_cancelled(&self, run_id: &str) -> bool {
        self.find(run_id)
            .is_some_and(|run| run.status == FleetRunStatus::Cancelled)
    }

    pub(crate) fn cancel(&mut self, target: &str) -> Option<FleetRunSnapshot> {
        let run = self.runs.iter_mut().find(|run| {
            short_or_full_id_matches(&run.run_id, target)
                || run
                    .workers
                    .iter()
                    .any(|worker| short_or_full_id_matches(&worker.id, target))
        })?;
        if short_or_full_id_matches(&run.run_id, target) {
            run.status = FleetRunStatus::Cancelled;
            run.finished_at_ms = Some(now_ms());
            run.final_summary = format!("Fleet run {} was cancelled.", short_id(&run.run_id));
            for worker in &mut run.workers {
                if worker.status == FleetWorkerStatus::Queued
                    || worker.status == FleetWorkerStatus::Running
                {
                    worker.status = FleetWorkerStatus::Cancelled;
                }
            }
        } else if let Some(worker) = run
            .workers
            .iter_mut()
            .find(|worker| short_or_full_id_matches(&worker.id, target))
        {
            worker.status = FleetWorkerStatus::Cancelled;
            worker.summary = "cancel requested".to_string();
        }
        Some(run.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FleetWorkerTurnRequest {
    pub(crate) prompt: String,
    pub(crate) timeout: Duration,
    pub(crate) local_image_paths: Vec<PathBuf>,
    pub(crate) remote_image_urls: Vec<String>,
    pub(crate) cancellation_token: CancellationToken,
}

pub(crate) trait FleetWorkflowRunner: Send + Sync {
    fn run_turn(
        &self,
        request: FleetWorkerTurnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FleetWorkerTurnResult, String>> + Send>>;
}

#[derive(Debug, Clone)]
pub(crate) struct FleetWorkerTurnResult {
    pub(crate) text: String,
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) duration_ms: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct AppServerFleetWorkflowRunner {
    request_handle: AppServerRequestHandle,
    config: Config,
    thread_params_mode: ThreadParamsMode,
    remote_cwd_override: Option<PathBuf>,
}

impl AppServerFleetWorkflowRunner {
    pub(crate) fn new(
        request_handle: AppServerRequestHandle,
        config: Config,
        thread_params_mode: ThreadParamsMode,
        remote_cwd_override: Option<PathBuf>,
    ) -> Self {
        Self {
            request_handle,
            config,
            thread_params_mode,
            remote_cwd_override,
        }
    }
}

impl FleetWorkflowRunner for AppServerFleetWorkflowRunner {
    fn run_turn(
        &self,
        request: FleetWorkerTurnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FleetWorkerTurnResult, String>> + Send>> {
        let request_handle = self.request_handle.clone();
        let config = self.config.clone();
        let thread_params_mode = self.thread_params_mode;
        let remote_cwd_override = self.remote_cwd_override.clone();
        Box::pin(async move {
            run_read_only_fleet_worker_turn(
                request_handle,
                config,
                thread_params_mode,
                remote_cwd_override,
                request.prompt,
                request.local_image_paths,
                request.remote_image_urls,
                request.timeout,
                request.cancellation_token,
            )
            .await
            .map(fleet_turn_result_from_app_server)
            .map_err(|err| err.to_string())
        })
    }
}

fn fleet_turn_result_from_app_server(
    result: AppServerFleetWorkerTurnResult,
) -> FleetWorkerTurnResult {
    FleetWorkerTurnResult {
        text: result.text,
        thread_id: Some(result.thread_id.to_string()),
        turn_id: Some(result.turn_id),
        duration_ms: result.duration_ms,
    }
}

pub(crate) fn fleet_persistence_dir(config: &Config) -> PathBuf {
    config.codex_home.join("fleet").join("runs").to_path_buf()
}

#[derive(Debug, Clone, Deserialize)]
struct ModelFleetPlan {
    max_concurrency: Option<usize>,
    workers: Vec<ModelFleetWorker>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelFleetWorker {
    id: Option<String>,
    role: Option<String>,
    goal: String,
    depends_on: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct FleetWorkerSpec {
    id: String,
    role: FleetWorkerRole,
    goal: String,
    depends_on: Vec<String>,
}

#[derive(Debug, Clone)]
struct FleetPlan {
    max_concurrency: usize,
    workers: Vec<FleetWorkerSpec>,
    planner_notes: String,
}

pub(crate) fn initial_snapshot(request: &FleetRunRequest) -> FleetRunSnapshot {
    FleetRunSnapshot {
        run_id: request.run_id.clone(),
        task: request.task.clone(),
        status: FleetRunStatus::Planning,
        max_concurrency: request.max_concurrency,
        associated_goal_objective: request.associated_goal_objective.clone(),
        started_at_ms: now_ms(),
        finished_at_ms: None,
        workers: Vec::new(),
        final_summary: String::new(),
        planner_notes: String::new(),
    }
}

pub(crate) async fn run_fleet_workflow(
    request: FleetRunRequest,
    runner: Arc<dyn FleetWorkflowRunner>,
    cancellation_token: CancellationToken,
    updates: Option<mpsc::UnboundedSender<FleetRunSnapshot>>,
) -> FleetRunSnapshot {
    let mut snapshot = initial_snapshot(&request);
    persist_and_send(&request, &updates, &snapshot);

    let plan = plan_workflow(&request, runner.as_ref(), cancellation_token.clone()).await;
    if cancellation_token.is_cancelled() {
        snapshot.status = FleetRunStatus::Cancelled;
        snapshot.finished_at_ms = Some(now_ms());
        snapshot.final_summary = format!("Fleet run {} was cancelled.", short_id(&snapshot.run_id));
        persist_and_send(&request, &updates, &snapshot);
        return snapshot;
    }
    snapshot.max_concurrency = plan.max_concurrency;
    snapshot.planner_notes = plan.planner_notes.clone();
    snapshot.status = FleetRunStatus::Running;
    snapshot.workers = plan.workers.iter().map(worker_from_spec).collect();
    persist_and_send(&request, &updates, &snapshot);

    let completed_outputs = execute_workers(
        &request,
        runner,
        cancellation_token.clone(),
        &mut snapshot,
        &updates,
    )
    .await;
    let failed = snapshot
        .workers
        .iter()
        .any(|worker| worker.status == FleetWorkerStatus::Failed);
    let synthesizer_failed = snapshot.workers.iter().any(|worker| {
        worker.role == FleetWorkerRole::Synthesizer && worker.status != FleetWorkerStatus::Completed
    });
    snapshot.status = if cancellation_token.is_cancelled() {
        FleetRunStatus::Cancelled
    } else if failed || synthesizer_failed || completed_outputs.is_empty() {
        FleetRunStatus::Failed
    } else {
        FleetRunStatus::Completed
    };
    snapshot.finished_at_ms = Some(now_ms());
    snapshot.final_summary = build_final_summary(&snapshot);
    persist_and_send(&request, &updates, &snapshot);
    snapshot
}

async fn execute_workers(
    request: &FleetRunRequest,
    runner: Arc<dyn FleetWorkflowRunner>,
    cancellation_token: CancellationToken,
    snapshot: &mut FleetRunSnapshot,
    updates: &Option<mpsc::UnboundedSender<FleetRunSnapshot>>,
) -> HashMap<String, String> {
    let mut completed_outputs = HashMap::<String, String>::new();
    let mut active = tokio::task::JoinSet::<(usize, Result<FleetWorkerTurnResult, String>)>::new();

    loop {
        if cancellation_token.is_cancelled() {
            cancel_queued_and_running_workers(snapshot);
            persist_and_send(request, updates, snapshot);
            break;
        }
        while active.len() < snapshot.max_concurrency {
            if cancellation_token.is_cancelled() {
                break;
            }
            let Some(worker_index) = next_ready_worker(snapshot, &completed_outputs) else {
                break;
            };
            snapshot.workers[worker_index].status = FleetWorkerStatus::Running;
            persist_and_send(request, updates, snapshot);
            let prompt =
                worker_prompt(request, &snapshot.workers[worker_index], &completed_outputs);
            let runner = Arc::clone(&runner);
            let cancellation_token = cancellation_token.clone();
            let local_image_paths = request.local_image_paths.clone();
            let remote_image_urls = request.remote_image_urls.clone();
            active.spawn(async move {
                let result = runner
                    .run_turn(FleetWorkerTurnRequest {
                        prompt,
                        timeout: WORKER_TIMEOUT,
                        local_image_paths,
                        remote_image_urls,
                        cancellation_token,
                    })
                    .await;
                (worker_index, result)
            });
        }

        if active.is_empty() {
            fail_remaining_queued_workers(snapshot);
            persist_and_send(request, updates, snapshot);
            break;
        }

        let Some(join_result) = active.join_next().await else {
            break;
        };
        match join_result {
            Ok((worker_index, Ok(result))) => {
                let worker = &mut snapshot.workers[worker_index];
                worker.status = FleetWorkerStatus::Completed;
                worker.thread_id = result.thread_id;
                worker.turn_id = result.turn_id;
                worker.duration_ms = result.duration_ms;
                worker.summary = first_paragraph(&result.text);
                worker.details = result.text.clone();
                completed_outputs.insert(worker.id.clone(), result.text);
            }
            Ok((worker_index, Err(err))) => {
                let worker = &mut snapshot.workers[worker_index];
                worker.status = FleetWorkerStatus::Failed;
                worker.summary = err.clone();
                worker.details = err;
            }
            Err(err) => {
                if let Some(worker) = snapshot
                    .workers
                    .iter_mut()
                    .find(|worker| worker.status == FleetWorkerStatus::Running)
                {
                    worker.status = FleetWorkerStatus::Failed;
                    worker.summary = err.to_string();
                    worker.details = err.to_string();
                }
            }
        }
        persist_and_send(request, updates, snapshot);
    }

    completed_outputs
}

fn next_ready_worker(
    snapshot: &FleetRunSnapshot,
    completed_outputs: &HashMap<String, String>,
) -> Option<usize> {
    snapshot
        .workers
        .iter()
        .enumerate()
        .find(|(_, worker)| {
            worker.status == FleetWorkerStatus::Queued
                && worker
                    .depends_on
                    .iter()
                    .all(|dependency| completed_outputs.contains_key(dependency))
        })
        .map(|(index, _)| index)
}

fn fail_remaining_queued_workers(snapshot: &mut FleetRunSnapshot) {
    for worker in &mut snapshot.workers {
        if worker.status == FleetWorkerStatus::Queued {
            worker.status = FleetWorkerStatus::Failed;
            worker.summary = "dependencies did not complete".to_string();
            worker.details =
                "Worker could not start because one or more dependencies failed.".to_string();
        }
    }
}

fn cancel_queued_and_running_workers(snapshot: &mut FleetRunSnapshot) {
    for worker in &mut snapshot.workers {
        if worker.status == FleetWorkerStatus::Queued || worker.status == FleetWorkerStatus::Running
        {
            worker.status = FleetWorkerStatus::Cancelled;
            worker.summary = "cancelled".to_string();
            worker.details = "Fleet run cancellation requested.".to_string();
        }
    }
}

fn worker_prompt(
    request: &FleetRunRequest,
    worker: &FleetWorkerSnapshot,
    completed_outputs: &HashMap<String, String>,
) -> String {
    let dependencies = worker
        .depends_on
        .iter()
        .filter_map(|dependency| {
            completed_outputs
                .get(dependency)
                .map(|output| (dependency, output))
        })
        .map(|(dependency, output)| {
            format!(
                "## {dependency}\n{}",
                truncate_for_prompt(output, DEPENDENCY_OUTPUT_LIMIT_CHARS)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        r#"Overall /fleet task:
{task}

Worker id: {id}
Worker role: {role}
Assigned goal:
{goal}

Rich input context:
{rich_context}

Read-only rules:
- Do not edit files, apply patches, change git state, send messages, update memory, or request broader permissions.
- Use available read/search/analysis tools as needed.
- For non-code research tasks, prefer web/search/current-data tools when available. Do not inspect repository files unless the assigned goal is about local code, local docs, or workspace artifacts.
- If current data or web access is unavailable, say so explicitly instead of fabricating sources.
- Return concise findings with evidence, confidence, and open questions.
- If this is the synthesizer, produce the final answer directly for the user.

Dependency outputs:
{dependencies}
"#,
        task = request.task,
        id = worker.id,
        role = worker_role_label(&worker.role),
        goal = worker.goal,
        rich_context = rich_input_context(request),
        dependencies = if dependencies.is_empty() {
            "<none>".to_string()
        } else {
            dependencies
        },
    )
}

async fn plan_workflow(
    request: &FleetRunRequest,
    runner: &dyn FleetWorkflowRunner,
    cancellation_token: CancellationToken,
) -> FleetPlan {
    match runner
        .run_turn(FleetWorkerTurnRequest {
            prompt: planner_prompt(request),
            timeout: PLANNER_TIMEOUT,
            local_image_paths: request.local_image_paths.clone(),
            remote_image_urls: request.remote_image_urls.clone(),
            cancellation_token,
        })
        .await
    {
        Ok(result) => match parse_model_plan(&result.text, request) {
            Ok(plan) => plan,
            Err(err) => {
                deterministic_plan(request, format!("model planner output rejected: {err}"))
            }
        },
        Err(err) => deterministic_plan(request, format!("model planner failed: {err}")),
    }
}

fn planner_prompt(request: &FleetRunRequest) -> String {
    format!(
        r#"You are the /fleet dynamic workflow planner for Codepilot.
Return ONLY valid JSON. Do not include markdown fences or prose.
Create a read-only workflow graph for this task:

{task}

Rich input context:
{rich_context}

Schema:
{{"max_concurrency": 1-8, "workers": [{{"id":"kebab-id", "role":"researcher|implementer|verifier|critic|synthesizer", "goal":"specific worker goal", "depends_on":["other-id"]}}]}}

Rules:
- Use enough workers for meaningfully distinct lanes; do not hard-code a fixed count.
- Include exactly one synthesizer worker that depends on the important workers.
- Keep all workers read-only; workers may inspect, search, and analyze but must not edit.
- Prefer verifier/critic workers for code review, debugging, and risk-heavy tasks.
- For market, fund, investment, policy, or other non-code research, create domain expert lanes such as macro, sector, product universe, risk, and philosophy-specific panelists. Do not use repository/file-inspection lanes unless the user asks about code or local files.
- For current-data research, workers should use web/search tools if available and explicitly report when fresh data is unavailable.
- Keep total workers at or below {hard_max_workers}.
"#,
        task = request.task,
        rich_context = rich_input_context(request),
        hard_max_workers = HARD_MAX_WORKERS
    )
}

fn parse_model_plan(content: &str, request: &FleetRunRequest) -> Result<FleetPlan, String> {
    let json = extract_json_object(content)?;
    let parsed = serde_json::from_str::<ModelFleetPlan>(&json).map_err(|err| err.to_string())?;
    normalize_model_plan(parsed, request)
}

fn normalize_model_plan(
    parsed: ModelFleetPlan,
    request: &FleetRunRequest,
) -> Result<FleetPlan, String> {
    if parsed.workers.is_empty() {
        return Err("planner returned no workers".to_string());
    }
    let mut workers = Vec::new();
    let mut used = HashSet::new();
    for (index, worker) in parsed
        .workers
        .into_iter()
        .take(HARD_MAX_WORKERS)
        .enumerate()
    {
        let id = unique_worker_id(
            worker.id.as_deref().unwrap_or(&worker.goal),
            &mut used,
            index,
        );
        let role = parse_worker_role(worker.role.as_deref())?;
        let depends_on = worker
            .depends_on
            .unwrap_or_default()
            .into_iter()
            .map(|dependency| sanitize_worker_id(&dependency))
            .filter(|dependency| !dependency.is_empty())
            .collect::<Vec<_>>();
        workers.push(FleetWorkerSpec {
            id,
            role,
            goal: worker.goal.trim().to_string(),
            depends_on,
        });
    }
    validate_workers(&workers)?;
    ensure_synthesizer(&mut workers);
    validate_workers(&workers)?;
    Ok(FleetPlan {
        max_concurrency: parsed
            .max_concurrency
            .filter(|value| *value > 0)
            .unwrap_or(request.max_concurrency)
            .min(request.max_concurrency.max(1))
            .min(workers.len().max(1)),
        workers,
        planner_notes: "model-generated IR accepted".to_string(),
    })
}

fn deterministic_plan(request: &FleetRunRequest, planner_notes: String) -> FleetPlan {
    let lower = request.task.to_ascii_lowercase();
    let is_code_task = lower.contains("code")
        || lower.contains("review")
        || lower.contains("bug")
        || lower.contains("debug")
        || lower.contains("repo")
        || lower.contains("test")
        || lower.contains("implement");
    let is_market_research = lower.contains("fund")
        || lower.contains("market")
        || lower.contains("stock")
        || lower.contains("investment")
        || contains_word(&lower, "invest")
        || contains_word(&lower, "investing")
        || contains_word(&lower, "investor")
        || contains_word(&lower, "investors")
        || lower.contains("macro")
        || lower.contains("sector")
        || lower.contains("portfolio")
        || lower.contains("基金")
        || lower.contains("市场")
        || lower.contains("投资");
    let mut workers = if is_market_research {
        vec![
            FleetWorkerSpec {
                id: "macro-policy".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Research the current macro, policy, liquidity, and market backdrop relevant to the requested market or fund decision. Use web/search tools if available; otherwise clearly label freshness limits.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "fund-universe".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Identify the relevant fund/product universe, categories, constraints, and concrete candidates for the requested market. Use current data when available and separate verified facts from assumptions.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "value-income-panelist".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Analyze the request from a value/income-oriented investor perspective, including valuation, cash-flow, dividend, bond, or money-market considerations as applicable.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "growth-momentum-panelist".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Analyze the request from a growth/momentum investor perspective, including sector leadership, earnings expectations, flows, and catalysts as applicable.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "risk-officer".to_string(),
                role: FleetWorkerRole::Critic,
                goal: "Challenge the candidate recommendations, identify drawdown, concentration, liquidity, FX, policy, and data-quality risks, and state what should not be bought without further checks.".to_string(),
                depends_on: Vec::new(),
            },
        ]
    } else if is_code_task {
        vec![
            FleetWorkerSpec {
                id: "code-map".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Map the relevant code paths, files, existing behavior, and integration points for the requested coding task.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "regression-review".to_string(),
                role: FleetWorkerRole::Critic,
                goal: "Review implementation risks, regressions, and edge cases for the requested coding task.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "validation".to_string(),
                role: FleetWorkerRole::Verifier,
                goal: "Inspect validation strategy, test coverage, and likely checks for the requested coding task.".to_string(),
                depends_on: Vec::new(),
            },
        ]
    } else {
        vec![
            FleetWorkerSpec {
                id: "task-analysis".to_string(),
                role: FleetWorkerRole::Researcher,
                goal: "Analyze the user's task, identify key subquestions, gather relevant evidence, and state assumptions.".to_string(),
                depends_on: Vec::new(),
            },
            FleetWorkerSpec {
                id: "risk-review".to_string(),
                role: FleetWorkerRole::Critic,
                goal: "Challenge the likely answer, identify gaps, risks, and verification needs.".to_string(),
                depends_on: Vec::new(),
            },
        ]
    };
    if !is_market_research
        && lower.contains("research")
        && !workers.iter().any(|worker| worker.id == "research")
    {
        workers.push(FleetWorkerSpec {
            id: "research".to_string(),
            role: FleetWorkerRole::Verifier,
            goal: "Research external context and separate verified facts from assumptions. Use web/search tools if available and report limitations.".to_string(),
            depends_on: Vec::new(),
        });
    }
    ensure_synthesizer(&mut workers);
    FleetPlan {
        max_concurrency: request.max_concurrency.min(workers.len().max(1)),
        workers,
        planner_notes,
    }
}

fn ensure_synthesizer(workers: &mut Vec<FleetWorkerSpec>) {
    let dependencies = workers
        .iter()
        .filter(|worker| worker.role != FleetWorkerRole::Synthesizer)
        .map(|worker| worker.id.clone())
        .collect::<Vec<_>>();
    if let Some(synthesizer) = workers
        .iter_mut()
        .find(|worker| worker.role == FleetWorkerRole::Synthesizer)
    {
        if synthesizer.depends_on.is_empty() {
            synthesizer.depends_on = dependencies;
        }
        return;
    }
    workers.push(FleetWorkerSpec {
        id: "synthesizer".to_string(),
        role: FleetWorkerRole::Synthesizer,
        goal: "Synthesize worker outputs into the final user-facing result".to_string(),
        depends_on: dependencies,
    });
}

fn validate_workers(workers: &[FleetWorkerSpec]) -> Result<(), String> {
    let ids = workers
        .iter()
        .map(|worker| worker.id.as_str())
        .collect::<HashSet<_>>();
    if ids.len() != workers.len() {
        return Err("worker ids must be unique".to_string());
    }
    let synthesizers = workers
        .iter()
        .filter(|worker| worker.role == FleetWorkerRole::Synthesizer)
        .count();
    if synthesizers > 1 {
        return Err("workflow must not contain more than one synthesizer".to_string());
    }
    for worker in workers {
        if worker.goal.trim().is_empty() {
            return Err(format!("worker {} has an empty goal", worker.id));
        }
        for dependency in &worker.depends_on {
            if !ids.contains(dependency.as_str()) {
                return Err(format!(
                    "worker {} depends on unknown worker {dependency}",
                    worker.id
                ));
            }
        }
    }
    for worker in workers {
        let mut visiting = HashSet::new();
        if has_cycle(
            worker.id.as_str(),
            workers,
            &mut visiting,
            &mut HashSet::new(),
        ) {
            return Err("workflow dependencies must be acyclic".to_string());
        }
    }
    Ok(())
}

fn has_cycle(
    id: &str,
    workers: &[FleetWorkerSpec],
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) -> bool {
    if visited.contains(id) {
        return false;
    }
    if !visiting.insert(id.to_string()) {
        return true;
    }
    let Some(worker) = workers.iter().find(|worker| worker.id == id) else {
        return false;
    };
    for dependency in &worker.depends_on {
        if has_cycle(dependency, workers, visiting, visited) {
            return true;
        }
    }
    visiting.remove(id);
    visited.insert(id.to_string());
    false
}

fn parse_worker_role(role: Option<&str>) -> Result<FleetWorkerRole, String> {
    match role.unwrap_or("researcher").to_ascii_lowercase().as_str() {
        "researcher" | "research" => Ok(FleetWorkerRole::Researcher),
        "implementer" | "implementation" => Ok(FleetWorkerRole::Implementer),
        "verifier" | "verify" => Ok(FleetWorkerRole::Verifier),
        "critic" | "risk" => Ok(FleetWorkerRole::Critic),
        "synthesizer" | "synthesis" | "chair" => Ok(FleetWorkerRole::Synthesizer),
        other => Err(format!("unsupported worker role: {other}")),
    }
}

fn worker_from_spec(spec: &FleetWorkerSpec) -> FleetWorkerSnapshot {
    FleetWorkerSnapshot {
        id: spec.id.clone(),
        role: spec.role.clone(),
        goal: spec.goal.clone(),
        depends_on: spec.depends_on.clone(),
        status: FleetWorkerStatus::Queued,
        summary: String::new(),
        details: String::new(),
        thread_id: None,
        turn_id: None,
        duration_ms: None,
    }
}

fn build_final_summary(snapshot: &FleetRunSnapshot) -> String {
    if let Some(synthesizer) = snapshot.workers.iter().find(|worker| {
        worker.role == FleetWorkerRole::Synthesizer && worker.status == FleetWorkerStatus::Completed
    }) && !synthesizer.details.trim().is_empty()
    {
        return format!(
            "{}\n\nNo files were modified by advisory fleet workers. Use /fleet show {} for details.",
            synthesizer.details.trim(),
            short_id(&snapshot.run_id)
        );
    }
    let completed = snapshot
        .workers
        .iter()
        .filter(|worker| worker.status == FleetWorkerStatus::Completed)
        .count();
    let failed = snapshot
        .workers
        .iter()
        .filter(|worker| worker.status == FleetWorkerStatus::Failed)
        .count();
    let summaries = snapshot
        .workers
        .iter()
        .filter(|worker| !worker.summary.trim().is_empty())
        .map(|worker| format!("- {}: {}", worker.id, worker.summary))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Fleet workflow completed for: {}\nWorkers: {completed} completed, {failed} failed.\nNo files were modified by advisory fleet workers.\n{}\nUse /fleet show {} for details.",
        snapshot.task,
        summaries,
        short_id(&snapshot.run_id)
    )
}

fn first_paragraph(value: &str) -> String {
    value
        .split("\n\n")
        .next()
        .unwrap_or(value)
        .trim()
        .chars()
        .take(500)
        .collect()
}

fn rich_input_context(request: &FleetRunRequest) -> String {
    let mut lines = Vec::new();
    if !request.local_image_paths.is_empty() {
        lines.push(format!(
            "Local images: {}",
            request
                .local_image_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !request.remote_image_urls.is_empty() {
        lines.push(format!(
            "Remote images: {}",
            request.remote_image_urls.join(", ")
        ));
    }
    if !request.mention_context.is_empty() {
        lines.push(format!("Mentions: {}", request.mention_context.join("; ")));
    }
    if lines.is_empty() {
        "<none>".to_string()
    } else {
        lines.join("\n")
    }
}

fn truncate_for_prompt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let retained = text
        .chars()
        .take(max_chars.saturating_sub(120))
        .collect::<String>();
    format!(
        "{retained}\n\n[Truncated dependency output to {max_chars} characters before injecting into this worker prompt.]"
    )
}

fn persist_and_send(
    request: &FleetRunRequest,
    updates: &Option<mpsc::UnboundedSender<FleetRunSnapshot>>,
    snapshot: &FleetRunSnapshot,
) {
    if let Some(dir) = &request.persistence_dir
        && let Err(err) = save_snapshot(dir, snapshot)
    {
        tracing::warn!(
            "failed to persist fleet snapshot {}: {err}",
            snapshot.run_id
        );
    }
    if let Some(updates) = updates {
        let _ = updates.send(snapshot.clone());
    }
}

fn save_snapshot(dir: &std::path::Path, snapshot: &FleetRunSnapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", sanitize_worker_id(&snapshot.run_id)));
    let content = serde_json::to_string_pretty(snapshot).map_err(std::io::Error::other)?;
    std::fs::write(path, format!("{content}\n"))
}

fn extract_json_object(content: &str) -> Result<String, String> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = trimmed
        .find('{')
        .ok_or_else(|| "planner response did not contain JSON".to_string())?;
    let end = trimmed
        .rfind('}')
        .ok_or_else(|| "planner response did not contain JSON".to_string())?;
    Ok(trimmed[start..=end].to_string())
}

fn unique_worker_id(value: &str, used: &mut HashSet<String>, index: usize) -> String {
    let base = sanitize_worker_id(value);
    let base = if base.is_empty() {
        format!("worker-{}", index + 1)
    } else {
        base
    };
    let mut candidate = base.clone();
    let mut suffix = 2;
    while used.contains(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    used.insert(candidate.clone());
    candidate
}

fn sanitize_worker_id(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(48)
        .collect()
}

fn contains_word(text: &str, word: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == word)
}

pub(crate) fn fleet_status_params<'a>(
    runs: impl IntoIterator<Item = &'a FleetRunSnapshot>,
) -> SelectionViewParams {
    let mut items = Vec::new();
    for run in runs {
        items.push(run_item(run));
        for worker in &run.workers {
            items.push(worker_item(run, worker));
        }
    }
    if items.is_empty() {
        items.push(SelectionItem {
            name: "No fleet workflows yet.".to_string(),
            description: Some(
                "Run /fleet <task> to start async workflow orchestration.".to_string(),
            ),
            is_disabled: true,
            ..Default::default()
        });
    }

    SelectionViewParams {
        title: Some("Fleet Status".to_string()),
        subtitle: Some("Async workflow runs and worker lanes.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

pub(crate) fn fleet_run_detail_params(run: &FleetRunSnapshot) -> SelectionViewParams {
    let mut items = vec![
        detail_item("Task", run.task.clone()),
        detail_item("Status", run_status_label(&run.status).to_string()),
        detail_item("Summary", empty_marker(&run.final_summary)),
    ];
    if let Some(goal) = &run.associated_goal_objective {
        items.push(detail_item("Associated goal", goal.clone()));
    }
    for worker in &run.workers {
        items.push(detail_item(
            format!("{} ({})", worker.id, worker_role_label(&worker.role)),
            format!(
                "{} · {}\n{}",
                worker_status_label(&worker.status),
                worker.goal,
                empty_marker(&worker.details)
            ),
        ));
    }

    SelectionViewParams {
        title: Some(format!("Fleet Run {}", short_id(&run.run_id))),
        subtitle: Some(format!(
            "{} · {} workers · concurrency {}",
            run_status_label(&run.status),
            run.workers.len(),
            run.max_concurrency
        )),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

pub(crate) fn fleet_result_lines(snapshot: &FleetRunSnapshot) -> Vec<Line<'static>> {
    let completed = snapshot
        .workers
        .iter()
        .filter(|worker| worker.status == FleetWorkerStatus::Completed)
        .count();
    let failed = snapshot
        .workers
        .iter()
        .filter(|worker| worker.status == FleetWorkerStatus::Failed)
        .count();
    let mut lines = vec![
        Line::from(vec![
            "Fleet Result ".bold(),
            short_id(&snapshot.run_id).cyan(),
        ]),
        Line::from(format!("Status: {}", run_status_label(&snapshot.status))),
        Line::from(format!("Task: {}", snapshot.task)),
        Line::from(format!(
            "Workers: {completed} completed · {failed} failed · {} total",
            snapshot.workers.len()
        )),
    ];
    if let Some(goal) = &snapshot.associated_goal_objective {
        lines.push(Line::from(format!("Associated goal: {goal}")));
    }
    lines.push(Line::from(""));
    for line in snapshot.final_summary.lines() {
        lines.push(Line::from(line.to_string()));
    }
    lines.push(Line::from(format!(
        "Elapsed: {} ms",
        snapshot
            .finished_at_ms
            .unwrap_or_else(now_ms)
            .saturating_sub(snapshot.started_at_ms)
    )));
    lines.push(Line::from(format!(
        "Open details: /fleet show {}",
        short_id(&snapshot.run_id)
    )));
    lines
}

fn run_item(run: &FleetRunSnapshot) -> SelectionItem {
    let run_id = run.run_id.clone();
    SelectionItem {
        name: format!("Run {}", short_id(&run.run_id)),
        description: Some(format!(
            "{} · {} workers · {}",
            run_status_label(&run.status),
            run.workers.len(),
            truncate(&run.task, 100)
        )),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenFleetRunDetails {
                run_id: run_id.clone(),
            });
        })],
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {}",
            run.run_id,
            run_status_label(&run.status),
            run.task
        )),
        ..Default::default()
    }
}

fn worker_item(run: &FleetRunSnapshot, worker: &FleetWorkerSnapshot) -> SelectionItem {
    let run_id = run.run_id.clone();
    SelectionItem {
        name: format!("  {} [{}]", worker.id, worker_role_label(&worker.role)),
        description: Some(format!(
            "{} · {} · {}",
            worker_status_label(&worker.status),
            dependency_label(worker),
            truncate(&worker.goal, 100)
        )),
        actions: vec![Box::new(move |tx| {
            tx.send(AppEvent::OpenFleetRunDetails {
                run_id: run_id.clone(),
            });
        })],
        dismiss_on_select: true,
        search_value: Some(format!(
            "{} {} {} {}",
            run.run_id,
            worker.id,
            worker_role_label(&worker.role),
            worker.goal
        )),
        ..Default::default()
    }
}

fn detail_item(name: impl Into<String>, description: String) -> SelectionItem {
    SelectionItem {
        name: name.into(),
        description: Some(description),
        is_disabled: true,
        ..Default::default()
    }
}

fn dependency_label(worker: &FleetWorkerSnapshot) -> String {
    if worker.depends_on.is_empty() {
        "no deps".to_string()
    } else {
        format!("deps: {}", worker.depends_on.join(", "))
    }
}

fn run_status_label(status: &FleetRunStatus) -> &'static str {
    match status {
        FleetRunStatus::Planning => "planning",
        FleetRunStatus::Running => "running",
        FleetRunStatus::Completed => "completed",
        FleetRunStatus::Failed => "failed",
        FleetRunStatus::Cancelled => "cancelled",
    }
}

fn worker_status_label(status: &FleetWorkerStatus) -> &'static str {
    match status {
        FleetWorkerStatus::Queued => "queued",
        FleetWorkerStatus::Running => "running",
        FleetWorkerStatus::Completed => "completed",
        FleetWorkerStatus::Failed => "failed",
        FleetWorkerStatus::Cancelled => "cancelled",
    }
}

fn worker_role_label(role: &FleetWorkerRole) -> &'static str {
    match role {
        FleetWorkerRole::Researcher => "researcher",
        FleetWorkerRole::Implementer => "implementer",
        FleetWorkerRole::Verifier => "verifier",
        FleetWorkerRole::Critic => "critic",
        FleetWorkerRole::Synthesizer => "synthesizer",
    }
}

fn empty_marker(text: &str) -> String {
    if text.trim().is_empty() {
        "<empty>".to_string()
    } else {
        text.to_string()
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn short_or_full_id_matches(id: &str, candidate: &str) -> bool {
    id == candidate || id.starts_with(candidate)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Mutex;

    #[derive(Clone)]
    struct StubRunner {
        outputs: Arc<Mutex<Vec<Result<String, String>>>>,
        prompts: Arc<Mutex<Vec<String>>>,
        cancelled_seen: Arc<Mutex<Vec<bool>>>,
    }

    impl StubRunner {
        fn new(outputs: Vec<Result<String, String>>) -> Self {
            Self {
                outputs: Arc::new(Mutex::new(outputs)),
                prompts: Arc::new(Mutex::new(Vec::new())),
                cancelled_seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl FleetWorkflowRunner for StubRunner {
        fn run_turn(
            &self,
            request: FleetWorkerTurnRequest,
        ) -> Pin<Box<dyn Future<Output = Result<FleetWorkerTurnResult, String>> + Send>> {
            let outputs = Arc::clone(&self.outputs);
            let prompts = Arc::clone(&self.prompts);
            let cancelled_seen = Arc::clone(&self.cancelled_seen);
            Box::pin(async move {
                prompts
                    .lock()
                    .expect("prompts")
                    .push(request.prompt.clone());
                cancelled_seen
                    .lock()
                    .expect("cancelled")
                    .push(request.cancellation_token.is_cancelled());
                let text = outputs
                    .lock()
                    .expect("outputs")
                    .remove(0)
                    .map_err(|err| err.to_string())?;
                Ok(FleetWorkerTurnResult {
                    text,
                    thread_id: Some("thread".to_string()),
                    turn_id: Some("turn".to_string()),
                    duration_ms: Some(1),
                })
            })
        }
    }

    #[test]
    fn model_plan_validation_rejects_cycles() {
        let request = FleetRunRequest::new("review".to_string(), Some(2));
        let content = r#"{"workers":[{"id":"a","role":"researcher","goal":"A","depends_on":["b"]},{"id":"b","role":"verifier","goal":"B","depends_on":["a"]}]}"#;

        let err = parse_model_plan(content, &request).expect_err("cycle should fail");

        assert!(err.contains("acyclic"));
    }

    #[test]
    fn deterministic_market_research_plan_uses_domain_panel_lanes() {
        let request = FleetRunRequest::new(
            "research china fund market and convene expert investment panel".to_string(),
            Some(6),
        );

        let plan = deterministic_plan(&request, "fallback".to_string());
        let ids = plan
            .workers
            .iter()
            .map(|worker| worker.id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"macro-policy"));
        assert!(ids.contains(&"fund-universe"));
        assert!(ids.contains(&"value-income-panelist"));
        assert!(ids.contains(&"growth-momentum-panelist"));
        assert!(ids.contains(&"risk-officer"));
        assert!(!ids.contains(&"current-state"));
        assert!(!ids.contains(&"code-review"));
    }

    #[tokio::test]
    async fn workflow_uses_model_generated_plan_and_runner_outputs() {
        let request = FleetRunRequest::new("review".to_string(), Some(2));
        let runner = StubRunner::new(vec![
            Ok(r#"{"max_concurrency":2,"workers":[{"id":"alpha","role":"researcher","goal":"Inspect alpha","depends_on":[]},{"id":"chair","role":"synthesizer","goal":"Synthesize","depends_on":["alpha"]}]}"#.to_string()),
            Ok("alpha findings".to_string()),
            Ok("final synthesis".to_string()),
        ]);

        let snapshot = run_fleet_workflow(
            request,
            Arc::new(runner),
            CancellationToken::new(),
            /*updates*/ None,
        )
        .await;

        assert_eq!(snapshot.status, FleetRunStatus::Completed);
        assert_eq!(snapshot.workers.len(), 2);
        assert!(snapshot.final_summary.contains("final synthesis"));
        assert_eq!(snapshot.workers[0].thread_id.as_deref(), Some("thread"));
    }

    #[tokio::test]
    async fn workflow_marks_partial_worker_failure_as_failed() {
        let request = FleetRunRequest::new("review".to_string(), Some(2));
        let runner = StubRunner::new(vec![
            Ok(r#"{"max_concurrency":2,"workers":[{"id":"alpha","role":"researcher","goal":"Inspect alpha","depends_on":[]},{"id":"beta","role":"verifier","goal":"Inspect beta","depends_on":[]},{"id":"chair","role":"synthesizer","goal":"Synthesize","depends_on":["alpha","beta"]}]}"#.to_string()),
            Ok("alpha findings".to_string()),
            Err("beta failed".to_string()),
        ]);

        let snapshot = run_fleet_workflow(
            request,
            Arc::new(runner),
            CancellationToken::new(),
            /*updates*/ None,
        )
        .await;

        assert_eq!(snapshot.status, FleetRunStatus::Failed);
    }

    #[test]
    fn worker_prompt_caps_dependency_outputs() {
        let request = FleetRunRequest::new("synthesize".to_string(), Some(2));
        let worker = FleetWorkerSnapshot {
            id: "chair".to_string(),
            role: FleetWorkerRole::Synthesizer,
            goal: "Synthesize".to_string(),
            depends_on: vec!["alpha".to_string()],
            status: FleetWorkerStatus::Queued,
            summary: String::new(),
            details: String::new(),
            thread_id: None,
            turn_id: None,
            duration_ms: None,
        };
        let mut outputs = HashMap::new();
        outputs.insert(
            "alpha".to_string(),
            "x".repeat(DEPENDENCY_OUTPUT_LIMIT_CHARS + 500),
        );

        let prompt = worker_prompt(&request, &worker, &outputs);

        assert!(prompt.contains("Truncated dependency output"));
        assert!(prompt.len() < DEPENDENCY_OUTPUT_LIMIT_CHARS + 2_000);
    }

    #[test]
    fn rich_input_context_is_included_in_prompts() {
        let mut request = FleetRunRequest::new("analyze screenshot".to_string(), Some(2));
        request.local_image_paths = vec![PathBuf::from("C:\\tmp\\screen.png")];
        request.remote_image_urls = vec!["https://example.com/screen.png".to_string()];
        request.mention_context = vec!["figma -> app://figma".to_string()];
        let worker = FleetWorkerSnapshot {
            id: "visual".to_string(),
            role: FleetWorkerRole::Researcher,
            goal: "Analyze visual evidence".to_string(),
            depends_on: Vec::new(),
            status: FleetWorkerStatus::Queued,
            summary: String::new(),
            details: String::new(),
            thread_id: None,
            turn_id: None,
            duration_ms: None,
        };

        let planner = planner_prompt(&request);
        let worker_prompt = worker_prompt(&request, &worker, &HashMap::new());

        assert!(planner.contains("C:\\tmp\\screen.png"));
        assert!(worker_prompt.contains("https://example.com/screen.png"));
        assert!(worker_prompt.contains("figma -> app://figma"));
    }

    #[test]
    fn deterministic_investigate_bug_plan_uses_code_lanes() {
        let request = FleetRunRequest::new("investigate a bug in this repo".to_string(), Some(4));

        let plan = deterministic_plan(&request, "fallback".to_string());
        let ids = plan
            .workers
            .iter()
            .map(|worker| worker.id.as_str())
            .collect::<Vec<_>>();

        assert!(ids.contains(&"code-map"));
        assert!(ids.contains(&"regression-review"));
        assert!(!ids.contains(&"macro-policy"));
    }

    #[test]
    fn store_cancel_marks_run_and_queued_workers_cancelled() {
        let request = FleetRunRequest::new("review".to_string(), Some(2));
        let mut snapshot = initial_snapshot(&request);
        let plan = deterministic_plan(&request, "fallback".to_string());
        snapshot.workers = plan.workers.iter().map(worker_from_spec).collect();
        let mut store = FleetRunStore::default();
        store.upsert(snapshot.clone());

        let cancelled = store.cancel(&snapshot.run_id).expect("cancelled run");

        assert_eq!(cancelled.status, FleetRunStatus::Cancelled);
        assert!(
            cancelled
                .workers
                .iter()
                .all(|worker| worker.status == FleetWorkerStatus::Cancelled)
        );
    }
}
