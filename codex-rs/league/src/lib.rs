use agent_client_protocol::Agent;
use agent_client_protocol::ConnectionTo;
use agent_client_protocol::schema::Implementation;
use agent_client_protocol::schema::InitializeRequest;
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::RequestPermissionOutcome;
use agent_client_protocol::schema::RequestPermissionRequest;
use agent_client_protocol::schema::RequestPermissionResponse;
use codex_protocol::league::LeagueAgent;
use codex_protocol::league::LeagueAgentCapability;
use codex_protocol::league::LeagueAgentTransport;
use codex_protocol::league::LeagueMode;
use codex_protocol::league::LeaguePromptDelivery;
use codex_protocol::league::render_command;
use futures::future::join_all;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use uuid::Uuid;

pub const DEFAULT_AGENT_TIMEOUT_SECONDS: u64 = 600;
pub const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 65_536;

#[derive(Debug, Clone)]
pub struct LeagueRunRequest {
    pub run_id: String,
    pub task: String,
    pub mode: LeagueMode,
    pub cwd: PathBuf,
    pub agents: Vec<LeagueAgent>,
    pub timeout: Duration,
    pub output_limit_bytes: usize,
    pub source_bundle: Option<String>,
}

impl LeagueRunRequest {
    pub fn new(task: String, mode: LeagueMode, cwd: PathBuf, agents: Vec<LeagueAgent>) -> Self {
        Self {
            run_id: Uuid::new_v4().to_string(),
            task,
            mode,
            cwd,
            agents,
            timeout: Duration::from_secs(DEFAULT_AGENT_TIMEOUT_SECONDS),
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
            source_bundle: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeagueRunStatus {
    Pending,
    Probing,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeagueAgentStatus {
    Pending,
    Probing,
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeagueWebCapability {
    NotProbed,
    Probing,
    WebNative,
    WebFetchOnly,
    ProvidedSourcesOnly,
    Failed,
    Unknown,
}

impl LeagueWebCapability {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotProbed => "not probed",
            Self::Probing => "probing",
            Self::WebNative => "web available",
            Self::WebFetchOnly => "web fetch available",
            Self::ProvidedSourcesOnly => "provided sources only",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeagueAgentSnapshot {
    pub name: String,
    pub command: Vec<String>,
    pub transport: LeagueAgentTransport,
    pub prompt_delivery: LeaguePromptDelivery,
    pub status: LeagueAgentStatus,
    pub web_capability: LeagueWebCapability,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    pub stdout_preview: String,
    pub stderr_preview: String,
    pub output_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeagueRunSnapshot {
    pub run_id: String,
    pub task: String,
    pub mode: LeagueMode,
    pub status: LeagueRunStatus,
    pub cwd: PathBuf,
    pub agents: Vec<LeagueAgentSnapshot>,
}

pub fn initial_snapshot(request: &LeagueRunRequest) -> LeagueRunSnapshot {
    LeagueRunSnapshot {
        run_id: request.run_id.clone(),
        task: request.task.clone(),
        mode: request.mode,
        status: LeagueRunStatus::Pending,
        cwd: request.cwd.clone(),
        agents: request
            .agents
            .iter()
            .map(|agent| LeagueAgentSnapshot {
                name: agent.name.clone(),
                command: agent.command.clone(),
                transport: agent.transport,
                prompt_delivery: agent.prompt_delivery,
                status: LeagueAgentStatus::Pending,
                web_capability: LeagueWebCapability::NotProbed,
                exit_code: None,
                duration_ms: None,
                stdout_preview: String::new(),
                stderr_preview: String::new(),
                output_truncated: false,
            })
            .collect(),
    }
}

pub async fn run_league(
    request: LeagueRunRequest,
    updates: Option<mpsc::UnboundedSender<LeagueRunSnapshot>>,
) -> LeagueRunSnapshot {
    let mut probing = initial_snapshot(&request);
    probing.status = LeagueRunStatus::Probing;
    for agent in &mut probing.agents {
        agent.status = LeagueAgentStatus::Probing;
        agent.web_capability = LeagueWebCapability::Probing;
    }
    send_update(&updates, probing);

    let probe_futures = request
        .agents
        .iter()
        .cloned()
        .map(|agent| probe_agent_web_capability(agent, request.cwd.clone(), request.timeout));
    let probe_results = join_all(probe_futures).await;

    let mut running = initial_snapshot(&request);
    running.status = LeagueRunStatus::Running;
    for (agent, capability) in running.agents.iter_mut().zip(probe_results.iter()) {
        agent.status = LeagueAgentStatus::Running;
        agent.web_capability = *capability;
    }
    send_update(&updates, running);

    let task_futures =
        request
            .agents
            .iter()
            .cloned()
            .zip(probe_results)
            .map(|(agent, web_capability)| {
                let request = request.clone();
                async move { run_agent_task(&request, agent, web_capability).await }
            });
    let agent_results = join_all(task_futures).await;
    let any_completed = agent_results
        .iter()
        .any(|agent| agent.status == LeagueAgentStatus::Completed);

    let snapshot = LeagueRunSnapshot {
        run_id: request.run_id,
        task: request.task,
        mode: request.mode,
        status: if any_completed {
            LeagueRunStatus::Completed
        } else {
            LeagueRunStatus::Failed
        },
        cwd: request.cwd,
        agents: agent_results,
    };
    send_update(&updates, snapshot.clone());
    snapshot
}

fn send_update(
    updates: &Option<mpsc::UnboundedSender<LeagueRunSnapshot>>,
    snapshot: LeagueRunSnapshot,
) {
    if let Some(updates) = updates {
        let _ = updates.send(snapshot);
    }
}

async fn probe_agent_web_capability(
    agent: LeagueAgent,
    cwd: PathBuf,
    timeout: Duration,
) -> LeagueWebCapability {
    if agent
        .capabilities
        .iter()
        .any(|capability| matches!(capability, LeagueAgentCapability::WebNative))
    {
        return LeagueWebCapability::WebNative;
    }
    if agent
        .capabilities
        .iter()
        .any(|capability| matches!(capability, LeagueAgentCapability::WebFetchOnly))
    {
        return LeagueWebCapability::WebFetchOnly;
    }
    if agent
        .capabilities
        .iter()
        .any(|capability| matches!(capability, LeagueAgentCapability::ProvidedSourcesOnly))
    {
        return LeagueWebCapability::ProvidedSourcesOnly;
    }

    let probe_prompt = "You are testing whether this CLI session has web/search/fetch capability. Do not edit files or run shell commands. If a WebSearch/WebFetch/browser/web tool is available, use it to fetch https://www.anthropic.com/news, then print WEB_SEARCH_WORKED and the tool name. If no web tool is available, print WEB_SEARCH_UNAVAILABLE.";
    match run_command_with_prompt(&agent, &cwd, probe_prompt, timeout, 4096).await {
        Ok(output) => classify_web_capability(&output.stdout, &output.stderr),
        Err(_) => LeagueWebCapability::Failed,
    }
}

pub fn classify_web_capability(stdout: &str, stderr: &str) -> LeagueWebCapability {
    let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
    if combined.contains("web_search_worked") {
        if combined.contains("websearch") || combined.contains("web search") {
            LeagueWebCapability::WebNative
        } else {
            LeagueWebCapability::WebFetchOnly
        }
    } else if combined.contains("web_search_unavailable") {
        LeagueWebCapability::ProvidedSourcesOnly
    } else {
        LeagueWebCapability::Unknown
    }
}

async fn run_agent_task(
    request: &LeagueRunRequest,
    agent: LeagueAgent,
    web_capability: LeagueWebCapability,
) -> LeagueAgentSnapshot {
    let prompt = agent_task_prompt(request, &agent, web_capability);
    let started = Instant::now();
    match run_command_with_prompt(
        &agent,
        &request.cwd,
        &prompt,
        request.timeout,
        request.output_limit_bytes,
    )
    .await
    {
        Ok(output) => LeagueAgentSnapshot {
            name: agent.name,
            command: agent.command,
            transport: agent.transport,
            prompt_delivery: agent.prompt_delivery,
            status: if output.exit_code == Some(0) {
                LeagueAgentStatus::Completed
            } else {
                LeagueAgentStatus::Failed
            },
            web_capability,
            exit_code: output.exit_code,
            duration_ms: Some(started.elapsed().as_millis()),
            stdout_preview: output.stdout,
            stderr_preview: output.stderr,
            output_truncated: output.truncated,
        },
        Err(err) => LeagueAgentSnapshot {
            name: agent.name,
            command: agent.command,
            transport: agent.transport,
            prompt_delivery: agent.prompt_delivery,
            status: if err.is_timeout {
                LeagueAgentStatus::TimedOut
            } else {
                LeagueAgentStatus::Failed
            },
            web_capability,
            exit_code: None,
            duration_ms: Some(started.elapsed().as_millis()),
            stdout_preview: String::new(),
            stderr_preview: err.message,
            output_truncated: false,
        },
    }
}

fn agent_task_prompt(
    request: &LeagueRunRequest,
    agent: &LeagueAgent,
    web_capability: LeagueWebCapability,
) -> String {
    let source_bundle = request
        .source_bundle
        .as_deref()
        .unwrap_or("No Codepilot source bundle was pre-gathered for this run.");
    let web_guidance = match web_capability {
        LeagueWebCapability::WebNative | LeagueWebCapability::WebFetchOnly => {
            "You may use your available web/fetch tools for source-finding, but clearly report which tool you used."
        }
        _ => {
            "Use only the provided source bundle for factual/current claims. Label anything else as hypothesis or prior knowledge."
        }
    };
    format!(
        r#"You are acting as an external agent for Codepilot.

Agent: {agent_name}
Mode: {mode}
Repository cwd: {cwd}
Task: {task}

Rules:
- Follow the requested task mode. Do not run destructive commands, commit, or push unless Codepilot explicitly asks for that in the prompt.
- Return concise findings with evidence, confidence, and suggested verification steps.
- If blocked, explain the blocker and what Codepilot should check locally.
- {web_guidance}

Codepilot source bundle:
{source_bundle}
"#,
        agent_name = agent.name,
        mode = request.mode.as_str(),
        cwd = request.cwd.display(),
        task = request.task,
    )
}

#[derive(Debug)]
struct CommandOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    truncated: bool,
}

#[derive(Debug)]
struct CommandRunError {
    message: String,
    is_timeout: bool,
}

async fn run_command_with_prompt(
    agent: &LeagueAgent,
    cwd: &PathBuf,
    prompt: &str,
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<CommandOutput, CommandRunError> {
    match agent.transport {
        LeagueAgentTransport::Cli => {
            run_cli_command_with_prompt(agent, cwd, prompt, timeout, output_limit_bytes).await
        }
        LeagueAgentTransport::Acp => match tokio::time::timeout(
            timeout,
            run_acp_command_with_prompt(agent, cwd, prompt, output_limit_bytes),
        )
        .await
        {
            Ok(result) => match result {
                Ok(output) => Ok(output),
                Err(err) if agent.name == "copilot" => {
                    let fallback_agent = copilot_cli_fallback_agent();
                    let mut output = run_cli_command_with_prompt(
                        &fallback_agent,
                        cwd,
                        prompt,
                        timeout,
                        output_limit_bytes,
                    )
                    .await?;
                    output.stderr = format!(
                        "ACP transport failed; fell back to Copilot CLI prompt mode: {}\n{}",
                        err.message, output.stderr
                    );
                    Ok(output)
                }
                Err(err) => Err(err),
            },
            Err(_) => Err(CommandRunError {
                message: format!("agent timed out after {}s", timeout.as_secs()),
                is_timeout: true,
            }),
        },
    }
}

async fn run_cli_command_with_prompt(
    agent: &LeagueAgent,
    cwd: &PathBuf,
    prompt: &str,
    timeout: Duration,
    output_limit_bytes: usize,
) -> Result<CommandOutput, CommandRunError> {
    let invocation = prepare_invocation(agent, prompt).map_err(|err| CommandRunError {
        message: err.to_string(),
        is_timeout: false,
    })?;
    let mut command = Command::new(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if invocation.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }

    let mut child = command.spawn().map_err(|err| CommandRunError {
        message: format!("failed to spawn {}: {err}", render_command(&agent.command)),
        is_timeout: false,
    })?;
    if let Some(stdin) = invocation.stdin
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin
            .write_all(stdin.as_bytes())
            .await
            .map_err(|err| CommandRunError {
                message: format!("failed to write prompt to stdin: {err}"),
                is_timeout: false,
            })?;
    }

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| CommandRunError {
            message: format!("agent timed out after {}s", timeout.as_secs()),
            is_timeout: true,
        })?
        .map_err(|err| CommandRunError {
            message: format!("failed to wait for agent: {err}"),
            is_timeout: false,
        })?;

    let (stdout, stdout_truncated) = truncate_bytes(&output.stdout, output_limit_bytes);
    let (stderr, stderr_truncated) = truncate_bytes(&output.stderr, output_limit_bytes);
    Ok(CommandOutput {
        stdout,
        stderr,
        exit_code: output.status.code(),
        truncated: stdout_truncated || stderr_truncated,
    })
}

async fn run_acp_command_with_prompt(
    agent: &LeagueAgent,
    cwd: &PathBuf,
    prompt: &str,
    output_limit_bytes: usize,
) -> Result<CommandOutput, CommandRunError> {
    let acp_agent =
        agent_client_protocol::AcpAgent::from_args(agent.command.iter()).map_err(|err| {
            CommandRunError {
                message: format!(
                    "failed to prepare ACP agent {}: {err}",
                    render_command(&agent.command)
                ),
                is_timeout: false,
            }
        })?;
    let response = agent_client_protocol::Client
        .builder()
        .name("codepilot-league")
        .on_receive_request(
            async move |_request: RequestPermissionRequest, responder, _connection| {
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(acp_agent, async move |connection: ConnectionTo<Agent>| {
            connection
                .send_request(
                    InitializeRequest::new(ProtocolVersion::V1).client_info(
                        Implementation::new("codepilot-league", env!("CARGO_PKG_VERSION"))
                            .title("Codepilot League"),
                    ),
                )
                .block_task()
                .await?;
            connection
                .build_session(cwd)
                .block_task()
                .run_until(async |mut session| {
                    session.send_prompt(prompt)?;
                    session.read_to_string().await
                })
                .await
        })
        .await
        .map_err(|err| CommandRunError {
            message: format!(
                "ACP session failed for {}: {err}",
                render_command(&agent.command)
            ),
            is_timeout: false,
        })?;
    let (stdout, truncated) = truncate_bytes(response.as_bytes(), output_limit_bytes);
    Ok(CommandOutput {
        stdout,
        stderr: String::new(),
        exit_code: Some(0),
        truncated,
    })
}

fn copilot_cli_fallback_agent() -> LeagueAgent {
    LeagueAgent {
        name: "copilot".to_string(),
        command: vec![
            "copilot".to_string(),
            "--allow-all".to_string(),
            "--no-color".to_string(),
            "-s".to_string(),
            "-p".to_string(),
        ],
        transport: LeagueAgentTransport::Cli,
        prompt_delivery: LeaguePromptDelivery::Arg,
        prompt_arg: None,
        capabilities: vec![
            LeagueAgentCapability::Code,
            LeagueAgentCapability::ProvidedSourcesOnly,
        ],
    }
}

struct PreparedInvocation {
    program: String,
    args: Vec<String>,
    stdin: Option<String>,
    _prompt_file: Option<NamedTempFile>,
}

fn prepare_invocation(agent: &LeagueAgent, prompt: &str) -> anyhow::Result<PreparedInvocation> {
    let Some(program) = agent.command.first().cloned() else {
        anyhow::bail!("agent command cannot be empty");
    };
    let mut args = agent.command.iter().skip(1).cloned().collect::<Vec<_>>();
    let mut stdin = None;
    let mut prompt_file = None;
    match agent.prompt_delivery {
        LeaguePromptDelivery::Stdin => stdin = Some(prompt.to_string()),
        LeaguePromptDelivery::StdinFile => {
            if args.iter().any(|arg| arg.contains("{prompt_file}")) {
                let file = NamedTempFile::new()?;
                std::fs::write(file.path(), prompt)?;
                let path = file.path().display().to_string();
                for arg in &mut args {
                    *arg = arg.replace("{prompt_file}", &path);
                }
                prompt_file = Some(file);
            } else {
                stdin = Some(prompt.to_string());
            }
        }
        LeaguePromptDelivery::Arg => {
            if let Some(prompt_arg) = agent.prompt_arg.as_ref() {
                args.push(prompt_arg.clone());
            }
            args.push(prompt.to_string());
        }
        LeaguePromptDelivery::Placeholder => {
            if !replace_prompt_placeholder(&mut args, prompt) {
                anyhow::bail!("prompt_delivery=placeholder requires a {prompt} argument");
            }
        }
    }
    Ok(PreparedInvocation {
        program,
        args,
        stdin,
        _prompt_file: prompt_file,
    })
}

fn replace_prompt_placeholder(args: &mut [String], prompt: &str) -> bool {
    let mut replaced = false;
    for arg in args.iter_mut() {
        if arg.contains("{prompt}") {
            *arg = arg.replace("{prompt}", prompt);
            replaced = true;
        }
    }
    replaced
}

fn truncate_bytes(bytes: &[u8], limit: usize) -> (String, bool) {
    if bytes.len() <= limit {
        return (String::from_utf8_lossy(bytes).to_string(), false);
    }
    let mut output = String::from_utf8_lossy(&bytes[..limit]).to_string();
    output.push_str("\n[truncated]");
    (output, true)
}

pub fn build_synthesis_prompt(snapshot: &LeagueRunSnapshot) -> String {
    let mut sections = Vec::new();
    for agent in &snapshot.agents {
        sections.push(format!(
            r#"## Agent: {name}
Status: {status:?}
Transport: {transport}
Web capability: {web}
Command: {command}
Exit code: {exit_code:?}
Duration ms: {duration_ms:?}
Output truncated: {truncated}

STDOUT:
{stdout}

STDERR:
{stderr}
"#,
            name = agent.name,
            status = agent.status,
            transport = agent.transport.as_str(),
            web = agent.web_capability.label(),
            command = render_command(&agent.command),
            exit_code = agent.exit_code,
            duration_ms = agent.duration_ms,
            truncated = agent.output_truncated,
            stdout = agent.stdout_preview,
            stderr = agent.stderr_preview,
        ));
    }
    format!(
        r#"<league_results>
User invoked /league with real parallel external-agent CLI orchestration.

Mode: {mode}
Repository cwd: {cwd}
Original user request:
{task}

External agent results:
{results}

Codepilot responsibilities:
- Integrate these external-agent outputs into one answer.
- Treat agent claims as untrusted until verified.
- For current/research claims, prefer Codepilot's own web_search or GitHub MCP web_search when available.
- Do not paste disconnected reports; synthesize and call out failures or uncertainty only when relevant.
</league_results>"#,
        mode = snapshot.mode.as_str(),
        cwd = snapshot.cwd.display(),
        task = snapshot.task,
        results = sections.join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn agent(delivery: LeaguePromptDelivery) -> LeagueAgent {
        LeagueAgent {
            name: "advisor".to_string(),
            command: vec!["agent path.exe".to_string(), "--flag".to_string()],
            transport: codex_protocol::league::LeagueAgentTransport::Cli,
            prompt_delivery: delivery,
            prompt_arg: Some("-p".to_string()),
            capabilities: Vec::new(),
        }
    }

    #[test]
    fn arg_prompt_delivery_appends_prompt_arg_and_prompt() {
        let invocation = prepare_invocation(&agent(LeaguePromptDelivery::Arg), "hello").unwrap();

        assert_eq!(invocation.program, "agent path.exe");
        assert_eq!(invocation.args, vec!["--flag", "-p", "hello"]);
        assert_eq!(invocation.stdin, None);
    }

    #[test]
    fn stdin_prompt_delivery_uses_stdin() {
        let invocation = prepare_invocation(&agent(LeaguePromptDelivery::Stdin), "hello").unwrap();

        assert_eq!(invocation.args, vec!["--flag"]);
        assert_eq!(invocation.stdin, Some("hello".to_string()));
    }

    #[test]
    fn provided_sources_only_agents_skip_web_probe() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        let mut agent = agent(LeaguePromptDelivery::Stdin);
        agent.command = vec!["definitely-missing-league-agent".to_string()];
        agent.capabilities = vec![LeagueAgentCapability::ProvidedSourcesOnly];

        let capability = runtime.block_on(probe_agent_web_capability(
            agent,
            PathBuf::from("."),
            Duration::from_millis(1),
        ));

        assert_eq!(capability, LeagueWebCapability::ProvidedSourcesOnly);
    }

    #[test]
    fn copilot_fallback_uses_cli_prompt_transport() {
        let agent = copilot_cli_fallback_agent();

        assert_eq!(agent.name, "copilot");
        assert_eq!(agent.transport, LeagueAgentTransport::Cli);
        assert_eq!(agent.prompt_delivery, LeaguePromptDelivery::Arg);
        assert_eq!(
            agent.command,
            vec!["copilot", "--allow-all", "--no-color", "-s", "-p"]
        );
    }

    #[test]
    fn classifies_claude_webfetch_probe_success() {
        assert_eq!(
            classify_web_capability("WEB_SEARCH_WORKED via WebFetch", ""),
            LeagueWebCapability::WebFetchOnly
        );
    }

    #[test]
    fn render_command_quotes_paths_with_spaces() {
        assert_eq!(
            render_command(&[
                "C:\\Program Files\\agent.exe".to_string(),
                "--x".to_string()
            ]),
            "\"C:\\Program Files\\agent.exe\" --x"
        );
    }
}
