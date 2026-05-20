use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use codex_api::SharedAuthProvider;
use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;
use codex_login::github_copilot::GITHUB_COPILOT_EDITOR_PLUGIN_VERSION;
use codex_login::github_copilot::GITHUB_COPILOT_EDITOR_VERSION;
use codex_login::github_copilot::GITHUB_COPILOT_INTEGRATION_ID;
use codex_login::github_copilot::GITHUB_COPILOT_OPENAI_INTENT;
use codex_login::github_copilot::GITHUB_COPILOT_USER_AGENT;
use codex_login::github_copilot::load_or_refresh_github_copilot_auth;
use codex_login::github_copilot_storage::load_github_copilot_auth;
use codex_model_provider::BearerAuthProvider;

pub const GITHUB_COPILOT_MCP_SERVER_NAME: &str = "github-mcp-server";
pub const GITHUB_COPILOT_MCP_SERVER_URL: &str =
    "https://api.enterprise.githubcopilot.com/mcp/readonly";

pub fn github_copilot_mcp_auth_available(codex_home: &Path) -> bool {
    match load_github_copilot_auth(codex_home) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(error = %err, "failed to read GitHub Copilot auth for builtin MCP");
            false
        }
    }
}

pub fn github_copilot_mcp_server_config() -> McpServerConfig {
    McpServerConfig {
        transport: McpServerTransportConfig::StreamableHttp {
            url: GITHUB_COPILOT_MCP_SERVER_URL.to_string(),
            bearer_token_env_var: None,
            http_headers: Some(github_copilot_mcp_http_headers()),
            env_http_headers: None,
        },
        experimental_environment: None,
        enabled: true,
        required: false,
        supports_parallel_tool_calls: false,
        disabled_reason: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        default_tools_approval_mode: None,
        enabled_tools: None,
        disabled_tools: None,
        scopes: None,
        oauth: None,
        oauth_resource: None,
        tools: HashMap::new(),
    }
}

fn github_copilot_mcp_http_headers() -> HashMap<String, String> {
    HashMap::from([
        (
            "user-agent".to_string(),
            GITHUB_COPILOT_USER_AGENT.to_string(),
        ),
        (
            "editor-version".to_string(),
            GITHUB_COPILOT_EDITOR_VERSION.to_string(),
        ),
        (
            "editor-plugin-version".to_string(),
            GITHUB_COPILOT_EDITOR_PLUGIN_VERSION.to_string(),
        ),
        (
            "copilot-integration-id".to_string(),
            GITHUB_COPILOT_INTEGRATION_ID.to_string(),
        ),
        (
            "openai-intent".to_string(),
            GITHUB_COPILOT_OPENAI_INTENT.to_string(),
        ),
    ])
}

pub async fn github_copilot_mcp_auth_provider(
    codex_home: &Path,
) -> Result<Option<SharedAuthProvider>> {
    let Some(auth) = load_or_refresh_github_copilot_auth(codex_home).await? else {
        return Ok(None);
    };
    Ok(Some(Arc::new(BearerAuthProvider::new(
        auth.github_access_token,
    ))))
}

pub async fn github_copilot_mcp_runtime_auth_available(codex_home: &Path) -> bool {
    match github_copilot_mcp_auth_provider(codex_home).await {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(err) => {
            tracing::warn!(error = %err, "failed to load GitHub Copilot auth for builtin MCP");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::github_copilot_mcp_auth_provider;

    #[tokio::test]
    async fn github_copilot_mcp_auth_provider_uses_github_token() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            codex_home.path().join("github-copilot-auth.json"),
            json!({
                "github_access_token": "github-token",
                "copilot_access_token": "tid=1;proxy-ep=https%3A%2F%2Fapi.enterprise.githubcopilot.com;sku=x",
                "copilot_token_expires_at": "2999-01-01T00:00:00Z",
                "api_base_url": "https://api.enterprise.githubcopilot.com",
                "saved_at": "2026-01-01T00:00:00Z"
            })
            .to_string(),
        )
        .expect("write auth");

        let provider = github_copilot_mcp_auth_provider(codex_home.path())
            .await
            .expect("provider should load")
            .expect("provider should exist");

        let headers = provider.to_auth_headers();
        assert_eq!(
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer github-token")
        );
    }
}
