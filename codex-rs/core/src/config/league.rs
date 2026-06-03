use codex_config::config_toml::ConfigToml;
use codex_protocol::league::LeagueAgent;
use codex_protocol::league::LeagueAgentCapability;
use codex_protocol::league::LeagueAgentTransport;
use codex_protocol::league::LeaguePromptDelivery;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

use super::Config;

const DEFAULT_LEAGUE_MAX_AGENTS: usize = 6;
const DEFAULT_AGENT_TIMEOUT_SECONDS: u64 = 600;
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 65_536;
const DEFAULT_STATUS_RETENTION: usize = 20;
const BUILT_IN_LEAGUE_AGENTS: &[&str] = &["claude", "copilot", "codex"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueConfig {
    pub enabled: bool,
    pub default_agents: Option<Vec<String>>,
    pub disabled_agents: BTreeSet<String>,
    pub max_agents: usize,
    pub agent_timeout_seconds: u64,
    pub output_limit_bytes: usize,
    pub status_retention: usize,
    pub agents: BTreeMap<String, LeagueAgentConfig>,
}

impl Default for LeagueConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_agents: None,
            disabled_agents: BTreeSet::new(),
            max_agents: DEFAULT_LEAGUE_MAX_AGENTS,
            agent_timeout_seconds: DEFAULT_AGENT_TIMEOUT_SECONDS,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
            status_retention: DEFAULT_STATUS_RETENTION,
            agents: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueAgentConfig {
    pub command: Vec<String>,
    pub transport: LeagueAgentTransport,
    pub prompt_delivery: LeaguePromptDelivery,
    pub prompt_arg: Option<String>,
    pub capabilities: Vec<LeagueAgentCapability>,
}

pub fn resolve_league_config(cfg: &ConfigToml) -> io::Result<LeagueConfig> {
    let Some(league) = cfg.league.as_ref() else {
        return Ok(LeagueConfig::default());
    };

    let max_agents = league.max_agents.unwrap_or(DEFAULT_LEAGUE_MAX_AGENTS);
    if max_agents == 0 {
        return Err(invalid_input("league.max_agents must be at least 1"));
    }
    let agent_timeout_seconds = league
        .agent_timeout_seconds
        .unwrap_or(DEFAULT_AGENT_TIMEOUT_SECONDS);
    if agent_timeout_seconds == 0 {
        return Err(invalid_input(
            "league.agent_timeout_seconds must be at least 1",
        ));
    }
    let output_limit_bytes = league
        .output_limit_bytes
        .unwrap_or(DEFAULT_OUTPUT_LIMIT_BYTES);
    if output_limit_bytes == 0 {
        return Err(invalid_input(
            "league.output_limit_bytes must be at least 1",
        ));
    }
    let status_retention = league.status_retention.unwrap_or(DEFAULT_STATUS_RETENTION);
    if status_retention == 0 {
        return Err(invalid_input("league.status_retention must be at least 1"));
    }

    let mut agents = BTreeMap::new();
    for (raw_name, agent) in &league.agents {
        let name = normalize_agent_name(raw_name, "league.agents")?;
        let command = agent
            .command
            .as_ref()
            .map(|command| normalize_command(command, &format!("league.agents.{name}.command")))
            .transpose()?;
        let command_was_overridden = command.is_some();
        let prompt_arg = agent
            .prompt_arg
            .as_ref()
            .map(|arg| normalize_prompt_arg(arg, &format!("league.agents.{name}.prompt_arg")))
            .transpose()?;
        let mut config = built_in_agent_config(&name).unwrap_or_else(|| LeagueAgentConfig {
            command: Vec::new(),
            transport: LeagueAgentTransport::Cli,
            prompt_delivery: LeaguePromptDelivery::Stdin,
            prompt_arg: None,
            capabilities: vec![
                LeagueAgentCapability::Code,
                LeagueAgentCapability::ProvidedSourcesOnly,
            ],
        });
        if let Some(command) = command {
            config.command = command;
        }
        if let Some(transport) = agent.transport {
            config.transport = transport;
            if !command_was_overridden
                && let Some(transport_config) =
                    built_in_agent_config_for_transport(&name, transport)
            {
                config = transport_config;
            }
        }
        if let Some(prompt_delivery) = agent.prompt_delivery {
            config.prompt_delivery = prompt_delivery;
        }
        if prompt_arg.is_some() {
            config.prompt_arg = prompt_arg;
        }
        if let Some(capabilities) = agent.capabilities.clone() {
            config.capabilities = capabilities;
        }
        if config.command.is_empty() {
            continue;
        }
        agents.insert(name, config);
    }

    Ok(LeagueConfig {
        enabled: league.enabled.unwrap_or(true),
        default_agents: normalize_optional_agent_names(
            league.default_agents.as_deref(),
            "league.default_agents",
        )?,
        disabled_agents: normalize_optional_agent_names(
            league.disabled_agents.as_deref(),
            "league.disabled_agents",
        )?
        .unwrap_or_default()
        .into_iter()
        .collect(),
        max_agents,
        agent_timeout_seconds,
        output_limit_bytes,
        status_retention,
        agents,
    })
}

pub fn resolve_league_agents(
    config: &Config,
    requested_agents: Option<&[String]>,
) -> Vec<LeagueAgent> {
    if !config.league.enabled {
        return Vec::new();
    }

    let candidates = requested_agents
        .map(normalize_requested_agents)
        .unwrap_or_else(|| {
            config.league.default_agents.clone().unwrap_or_else(|| {
                BUILT_IN_LEAGUE_AGENTS
                    .iter()
                    .map(|agent| (*agent).to_string())
                    .collect()
            })
        });

    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();
    for name in candidates {
        if name.is_empty()
            || config.league.disabled_agents.contains(&name)
            || !seen.insert(name.clone())
        {
            continue;
        }
        let command = config
            .league
            .agents
            .get(&name)
            .map(|agent| agent.command.clone())
            .or_else(|| built_in_agent_config(&name).map(|agent| agent.command))
            .unwrap_or_else(|| vec![name.clone()]);
        let agent_config = config
            .league
            .agents
            .get(&name)
            .cloned()
            .or_else(|| built_in_agent_config(&name))
            .unwrap_or_else(|| LeagueAgentConfig {
                command: command.clone(),
                transport: LeagueAgentTransport::Cli,
                prompt_delivery: LeaguePromptDelivery::Stdin,
                prompt_arg: None,
                capabilities: vec![LeagueAgentCapability::ProvidedSourcesOnly],
            });
        if !command_is_available(&command) {
            continue;
        }
        resolved.push(LeagueAgent {
            name,
            command,
            transport: agent_config.transport,
            prompt_delivery: agent_config.prompt_delivery,
            prompt_arg: agent_config.prompt_arg,
            capabilities: agent_config.capabilities,
        });
        if resolved.len() >= config.league.max_agents {
            break;
        }
    }
    resolved
}

fn built_in_agent_config(name: &str) -> Option<LeagueAgentConfig> {
    if name == "copilot" {
        return built_in_acp_agent_config(name);
    }
    built_in_cli_agent_config(name)
}

fn built_in_agent_config_for_transport(
    name: &str,
    transport: LeagueAgentTransport,
) -> Option<LeagueAgentConfig> {
    match transport {
        LeagueAgentTransport::Cli => built_in_cli_agent_config(name),
        LeagueAgentTransport::Acp => built_in_acp_agent_config(name),
    }
}

fn built_in_cli_agent_config(name: &str) -> Option<LeagueAgentConfig> {
    match name {
        "claude" | "claude-code" => Some(LeagueAgentConfig {
            command: vec![
                "claude".to_string(),
                "--print".to_string(),
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
            transport: LeagueAgentTransport::Cli,
            prompt_delivery: LeaguePromptDelivery::Stdin,
            prompt_arg: None,
            capabilities: vec![
                LeagueAgentCapability::Code,
                LeagueAgentCapability::ProvidedSourcesOnly,
            ],
        }),
        "copilot" => Some(LeagueAgentConfig {
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
        }),
        "codex" => Some(LeagueAgentConfig {
            command: vec![
                "codex".to_string(),
                "--ask-for-approval".to_string(),
                "never".to_string(),
                "exec".to_string(),
                "-".to_string(),
            ],
            transport: LeagueAgentTransport::Cli,
            prompt_delivery: LeaguePromptDelivery::Stdin,
            prompt_arg: None,
            capabilities: vec![
                LeagueAgentCapability::Code,
                LeagueAgentCapability::ProvidedSourcesOnly,
            ],
        }),
        _ => None,
    }
}

fn built_in_acp_agent_config(name: &str) -> Option<LeagueAgentConfig> {
    match name {
        "copilot" => Some(LeagueAgentConfig {
            command: vec![
                "copilot".to_string(),
                "--acp".to_string(),
                "--stdio".to_string(),
            ],
            transport: LeagueAgentTransport::Acp,
            prompt_delivery: LeaguePromptDelivery::Stdin,
            prompt_arg: None,
            capabilities: vec![
                LeagueAgentCapability::Code,
                LeagueAgentCapability::ProvidedSourcesOnly,
            ],
        }),
        _ => None,
    }
}

fn normalize_optional_agent_names(
    names: Option<&[String]>,
    field_name: &str,
) -> io::Result<Option<Vec<String>>> {
    names
        .map(|names| {
            names
                .iter()
                .map(|name| normalize_agent_name(name, field_name))
                .collect::<io::Result<Vec<_>>>()
        })
        .transpose()
}

fn normalize_requested_agents(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| {
            let trimmed = name.trim().to_ascii_lowercase();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .collect()
}

fn normalize_agent_name(name: &str, field_name: &str) -> io::Result<String> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(invalid_input(format!(
            "{field_name} cannot contain empty agent names"
        )));
    }
    Ok(normalized)
}

fn normalize_command(command: &[String], field_name: &str) -> io::Result<Vec<String>> {
    let command = command
        .iter()
        .map(|part| part.trim().to_string())
        .collect::<Vec<_>>();
    if command.is_empty() || command.iter().any(std::string::String::is_empty) {
        return Err(invalid_input(format!("{field_name} cannot be empty")));
    }
    Ok(command)
}

fn normalize_prompt_arg(arg: &str, field_name: &str) -> io::Result<String> {
    let arg = arg.trim().to_string();
    if arg.is_empty() {
        return Err(invalid_input(format!("{field_name} cannot be empty")));
    }
    Ok(arg)
}

fn command_is_available(command: &[String]) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let path = Path::new(program);
    path.is_file() || which::which(program).is_ok()
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_config::config_toml::LeagueAgentToml;
    use codex_config::config_toml::LeagueToml;

    #[test]
    fn resolves_default_league_config() {
        let config = resolve_league_config(&ConfigToml::default()).expect("config");

        assert_eq!(config, LeagueConfig::default());
        assert_eq!(config.max_agents, 6);
    }

    #[test]
    fn builtin_codex_uses_non_interactive_exec() {
        let config = built_in_agent_config("codex").expect("built-in codex config");

        assert_eq!(
            config,
            LeagueAgentConfig {
                command: vec![
                    "codex".to_string(),
                    "--ask-for-approval".to_string(),
                    "never".to_string(),
                    "exec".to_string(),
                    "-".to_string(),
                ],
                transport: LeagueAgentTransport::Cli,
                prompt_delivery: LeaguePromptDelivery::Stdin,
                prompt_arg: None,
                capabilities: vec![
                    LeagueAgentCapability::Code,
                    LeagueAgentCapability::ProvidedSourcesOnly,
                ],
            }
        );
    }

    #[test]
    fn resolves_agent_lists_and_commands() {
        let config = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                enabled: Some(false),
                default_agents: Some(vec!["Claude".to_string(), "copilot".to_string()]),
                disabled_agents: Some(vec!["Aider".to_string()]),
                max_agents: Some(3),
                agent_timeout_seconds: Some(7),
                output_limit_bytes: Some(1234),
                status_retention: Some(5),
                agents: BTreeMap::from([(
                    "Claude".to_string(),
                    LeagueAgentToml {
                        command: Some(vec!["claude".to_string(), "-p".to_string()]),
                        transport: None,
                        prompt_delivery: None,
                        prompt_arg: None,
                        capabilities: None,
                    },
                )]),
            }),
            ..Default::default()
        })
        .expect("config");

        assert_eq!(
            config,
            LeagueConfig {
                enabled: false,
                default_agents: Some(vec!["claude".to_string(), "copilot".to_string()]),
                disabled_agents: BTreeSet::from(["aider".to_string()]),
                max_agents: 3,
                agent_timeout_seconds: 7,
                output_limit_bytes: 1234,
                status_retention: 5,
                agents: BTreeMap::from([(
                    "claude".to_string(),
                    LeagueAgentConfig {
                        command: vec!["claude".to_string(), "-p".to_string()],
                        transport: LeagueAgentTransport::Cli,
                        prompt_delivery: LeaguePromptDelivery::Stdin,
                        prompt_arg: None,
                        capabilities: vec![
                            LeagueAgentCapability::Code,
                            LeagueAgentCapability::ProvidedSourcesOnly,
                        ],
                    },
                )]),
            }
        );
    }

    #[test]
    fn resolves_copilot_acp_transport() {
        let config = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                agents: BTreeMap::from([(
                    "copilot".to_string(),
                    LeagueAgentToml {
                        command: None,
                        transport: Some(LeagueAgentTransport::Acp),
                        prompt_delivery: None,
                        prompt_arg: None,
                        capabilities: None,
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("config");

        assert_eq!(
            config.agents.get("copilot"),
            Some(&LeagueAgentConfig {
                command: vec![
                    "copilot".to_string(),
                    "--acp".to_string(),
                    "--stdio".to_string(),
                ],
                transport: LeagueAgentTransport::Acp,
                prompt_delivery: LeaguePromptDelivery::Stdin,
                prompt_arg: None,
                capabilities: vec![
                    LeagueAgentCapability::Code,
                    LeagueAgentCapability::ProvidedSourcesOnly,
                ],
            })
        );
    }

    #[test]
    fn resolves_copilot_to_acp_by_default() {
        let config = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                agents: BTreeMap::from([(
                    "copilot".to_string(),
                    LeagueAgentToml {
                        command: None,
                        transport: None,
                        prompt_delivery: None,
                        prompt_arg: None,
                        capabilities: None,
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("config");

        assert_eq!(
            config.agents.get("copilot"),
            Some(&LeagueAgentConfig {
                command: vec![
                    "copilot".to_string(),
                    "--acp".to_string(),
                    "--stdio".to_string(),
                ],
                transport: LeagueAgentTransport::Acp,
                prompt_delivery: LeaguePromptDelivery::Stdin,
                prompt_arg: None,
                capabilities: vec![
                    LeagueAgentCapability::Code,
                    LeagueAgentCapability::ProvidedSourcesOnly,
                ],
            })
        );
    }

    #[test]
    fn resolves_copilot_cli_transport_override() {
        let config = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                agents: BTreeMap::from([(
                    "copilot".to_string(),
                    LeagueAgentToml {
                        command: None,
                        transport: Some(LeagueAgentTransport::Cli),
                        prompt_delivery: None,
                        prompt_arg: None,
                        capabilities: None,
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect("config");

        assert_eq!(
            config.agents.get("copilot"),
            Some(&LeagueAgentConfig {
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
            })
        );
    }

    #[test]
    fn rejects_invalid_config_values() {
        let empty_agent = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                default_agents: Some(vec![" ".to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect_err("empty agent name should fail");
        assert_eq!(empty_agent.kind(), io::ErrorKind::InvalidInput);

        let empty_command = resolve_league_config(&ConfigToml {
            league: Some(LeagueToml {
                agents: BTreeMap::from([(
                    "claude".to_string(),
                    LeagueAgentToml {
                        command: Some(vec![" ".to_string()]),
                        transport: None,
                        prompt_delivery: None,
                        prompt_arg: None,
                        capabilities: None,
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        })
        .expect_err("empty command should fail");
        assert_eq!(empty_command.kind(), io::ErrorKind::InvalidInput);
    }
}
