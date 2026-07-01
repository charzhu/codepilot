use crate::auth::ExternalAuth;
use crate::auth::ExternalAuthFuture;
use crate::auth::ExternalAuthRefreshContext;
use crate::auth::ExternalAuthTokens;
use crate::default_client::build_reqwest_client;
use crate::github_copilot_storage::GitHubCopilotAuth;
use crate::github_copilot_storage::load_github_copilot_auth;
use crate::github_copilot_storage::save_github_copilot_auth;
use chrono::DateTime;
use chrono::TimeDelta;
use chrono::Utc;
use codex_protocol::auth::AuthMode;
use reqwest::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::Semaphore;
use tokio::time::Duration;

pub const DEFAULT_GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
pub const GITHUB_COPILOT_CLIENT_ID_ENV_VAR: &str = "CODEX_GITHUB_COPILOT_CLIENT_ID";
pub const GITHUB_COPILOT_ENTERPRISE_DOMAIN_ENV_VAR: &str = "CODEX_GITHUB_COPILOT_ENTERPRISE_DOMAIN";
pub const DEFAULT_GITHUB_COPILOT_API_BASE_URL: &str = "https://api.githubcopilot.com";
pub const GITHUB_COPILOT_USER_AGENT: &str = "GitHubCopilotChat/0.35.0";
pub const GITHUB_COPILOT_EDITOR_VERSION: &str = "vscode/1.107.0";
pub const GITHUB_COPILOT_EDITOR_PLUGIN_VERSION: &str = "copilot-chat/0.35.0";
pub const GITHUB_COPILOT_INTEGRATION_ID: &str = "vscode-chat";
pub const GITHUB_COPILOT_OPENAI_INTENT: &str = "conversation-edits";

const DEVICE_CODE_SCOPE: &str = "read:user";
const TOKEN_REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubCopilotAuthConfig {
    pub client_id: String,
    pub enterprise_domain: Option<String>,
}

impl GitHubCopilotAuthConfig {
    pub fn new(client_id: impl Into<String>, enterprise_domain: Option<String>) -> Self {
        Self {
            client_id: client_id.into(),
            enterprise_domain: enterprise_domain.map(normalize_enterprise_domain),
        }
    }

    pub fn from_env(enterprise_domain: Option<String>) -> Self {
        let client_id = std::env::var(GITHUB_COPILOT_CLIENT_ID_ENV_VAR)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_GITHUB_COPILOT_CLIENT_ID.to_string());
        let enterprise_domain = enterprise_domain.or_else(|| {
            std::env::var(GITHUB_COPILOT_ENTERPRISE_DOMAIN_ENV_VAR)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
        Self::new(client_id, enterprise_domain)
    }

    fn endpoints(&self) -> GitHubCopilotEndpoints {
        GitHubCopilotEndpoints::for_enterprise_domain(self.enterprise_domain.as_deref())
    }
}

impl Default for GitHubCopilotAuthConfig {
    fn default() -> Self {
        Self::from_env(/*enterprise_domain*/ None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubCopilotDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubCopilotEndpoints {
    device_code_url: String,
    access_token_url: String,
    copilot_token_url: String,
}

impl GitHubCopilotEndpoints {
    fn for_enterprise_domain(enterprise_domain: Option<&str>) -> Self {
        let Some(domain) = enterprise_domain else {
            return Self {
                device_code_url: "https://github.com/login/device/code".to_string(),
                access_token_url: "https://github.com/login/oauth/access_token".to_string(),
                copilot_token_url: "https://api.github.com/copilot_internal/v2/token".to_string(),
            };
        };

        let domain = normalize_enterprise_domain(domain.to_string());
        let api_domain = if domain.starts_with("api.") {
            domain.clone()
        } else {
            format!("api.{domain}")
        };
        Self {
            device_code_url: format!("https://{domain}/login/device/code"),
            access_token_url: format!("https://{domain}/login/oauth/access_token"),
            copilot_token_url: format!("https://{api_domain}/copilot_internal/v2/token"),
        }
    }
}

#[derive(Debug, Serialize)]
struct DeviceCodeRequest<'a> {
    client_id: &'a str,
    scope: &'a str,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Debug, Serialize)]
struct AccessTokenRequest<'a> {
    client_id: &'a str,
    device_code: &'a str,
    grant_type: &'a str,
}

#[derive(Debug, Deserialize)]
struct AccessTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: i64,
}

pub fn build_github_copilot_client() -> Client {
    build_reqwest_client()
}

pub async fn request_device_code(
    client: &Client,
    config: &GitHubCopilotAuthConfig,
) -> std::io::Result<GitHubCopilotDeviceCode> {
    let endpoints = config.endpoints();
    let response = client
        .post(endpoints.device_code_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, GITHUB_COPILOT_USER_AGENT)
        .form(&DeviceCodeRequest {
            client_id: &config.client_id,
            scope: DEVICE_CODE_SCOPE,
        })
        .send()
        .await
        .map_err(std::io::Error::other)?;

    let status = response.status();
    let body = response.text().await.map_err(std::io::Error::other)?;
    if !status.is_success() {
        return Err(std::io::Error::other(format!(
            "GitHub device-code request failed: {status}: {}",
            github_error_message(&body),
        )));
    }

    let response: DeviceCodeResponse =
        serde_json::from_str(&body).map_err(std::io::Error::other)?;
    Ok(GitHubCopilotDeviceCode {
        device_code: response.device_code,
        user_code: response.user_code,
        verification_uri: response.verification_uri,
        verification_uri_complete: response.verification_uri_complete,
        expires_in: response.expires_in,
        interval: response.interval.unwrap_or(5),
    })
}

pub async fn wait_for_device_flow(
    client: &Client,
    config: &GitHubCopilotAuthConfig,
    device_code: &GitHubCopilotDeviceCode,
) -> std::io::Result<GitHubCopilotAuth> {
    let endpoints = config.endpoints();
    let expires_at = tokio::time::Instant::now() + Duration::from_secs(device_code.expires_in);
    let mut interval = device_code.interval.max(1);

    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= expires_at {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "GitHub device authorization expired before completion.",
            ));
        }

        let response = client
            .post(endpoints.access_token_url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, GITHUB_COPILOT_USER_AGENT)
            .form(&AccessTokenRequest {
                client_id: &config.client_id,
                device_code: &device_code.device_code,
                grant_type: "urn:ietf:params:oauth:grant-type:device_code",
            })
            .send()
            .await
            .map_err(std::io::Error::other)?;

        let status = response.status();
        let body = response.text().await.map_err(std::io::Error::other)?;
        if !status.is_success() {
            return Err(std::io::Error::other(format!(
                "GitHub access-token request failed: {status}: {}",
                github_error_message(&body),
            )));
        }

        let response: AccessTokenResponse =
            serde_json::from_str(&body).map_err(std::io::Error::other)?;
        match response.error.as_deref() {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval += 5;
                continue;
            }
            Some(error) => {
                let description = response.error_description.as_deref().unwrap_or(error);
                return Err(std::io::Error::other(format!(
                    "GitHub device authorization failed: {description}"
                )));
            }
            None => {
                let Some(github_access_token) = response.access_token else {
                    return Err(std::io::Error::other(
                        "GitHub device authorization succeeded without an access token.",
                    ));
                };
                return exchange_github_token_for_copilot_token(
                    client,
                    &endpoints,
                    github_access_token,
                    config.enterprise_domain.clone(),
                )
                .await;
            }
        }
    }
}

pub async fn refresh_github_copilot_auth(
    client: &Client,
    current_auth: GitHubCopilotAuth,
) -> std::io::Result<GitHubCopilotAuth> {
    let endpoints =
        GitHubCopilotEndpoints::for_enterprise_domain(current_auth.enterprise_domain.as_deref());
    exchange_github_token_for_copilot_token(
        client,
        &endpoints,
        current_auth.github_access_token,
        current_auth.enterprise_domain,
    )
    .await
}

pub fn is_copilot_token_stale(auth: &GitHubCopilotAuth) -> bool {
    auth.copilot_token_expires_at <= Utc::now() + TimeDelta::seconds(TOKEN_REFRESH_SKEW_SECONDS)
}

async fn exchange_github_token_for_copilot_token(
    client: &Client,
    endpoints: &GitHubCopilotEndpoints,
    github_access_token: String,
    enterprise_domain: Option<String>,
) -> std::io::Result<GitHubCopilotAuth> {
    let response = client
        .get(endpoints.copilot_token_url.clone())
        .bearer_auth(&github_access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::USER_AGENT, GITHUB_COPILOT_USER_AGENT)
        .header("editor-version", GITHUB_COPILOT_EDITOR_VERSION)
        .header(
            "editor-plugin-version",
            GITHUB_COPILOT_EDITOR_PLUGIN_VERSION,
        )
        .header("copilot-integration-id", GITHUB_COPILOT_INTEGRATION_ID)
        .send()
        .await
        .map_err(std::io::Error::other)?;

    let status = response.status();
    let body = response.text().await.map_err(std::io::Error::other)?;
    if !status.is_success() {
        return Err(std::io::Error::other(copilot_token_error(status, &body)));
    }

    let response: CopilotTokenResponse =
        serde_json::from_str(&body).map_err(std::io::Error::other)?;
    let expires_at = DateTime::<Utc>::from_timestamp(response.expires_at, 0).ok_or_else(|| {
        std::io::Error::other(format!(
            "GitHub Copilot returned invalid token expiry: {}",
            response.expires_at,
        ))
    })?;
    let api_base_url = api_base_url_from_token(&response.token)
        .unwrap_or_else(|| default_api_base_url(enterprise_domain.as_deref()));

    Ok(GitHubCopilotAuth::new(
        github_access_token,
        response.token,
        expires_at,
        api_base_url,
        enterprise_domain,
    ))
}

fn copilot_token_error(status: StatusCode, body: &str) -> String {
    if status == StatusCode::FORBIDDEN {
        return format!(
            "GitHub Copilot token request failed: {status}: {}. Confirm that Copilot Chat is enabled for this account.",
            github_error_message(body),
        );
    }
    format!(
        "GitHub Copilot token request failed: {status}: {}",
        github_error_message(body),
    )
}

fn github_error_message(body: &str) -> String {
    let value = serde_json::from_str::<serde_json::Value>(body).ok();
    value
        .as_ref()
        .and_then(|value| value.get("error_description"))
        .or_else(|| value.as_ref().and_then(|value| value.get("message")))
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or(body)
        .to_string()
}

fn api_base_url_from_token(token: &str) -> Option<String> {
    token.split(';').find_map(|part| {
        let proxy_ep = part.strip_prefix("proxy-ep=")?;
        let decoded = urlencoding::decode(proxy_ep).ok()?;
        api_base_url_from_proxy_endpoint(decoded.as_ref())
    })
}

fn api_base_url_from_proxy_endpoint(proxy_ep: &str) -> Option<String> {
    let trimmed = proxy_ep.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }

    let api_host = if let Some(host) = trimmed.strip_prefix("proxy.") {
        format!("api.{host}")
    } else if trimmed.starts_with("api.") {
        trimmed.to_string()
    } else {
        format!("api.{trimmed}")
    };
    Some(format!("https://{api_host}"))
}

fn default_api_base_url(enterprise_domain: Option<&str>) -> String {
    match enterprise_domain {
        Some(domain) if !domain.trim().is_empty() => format!("https://copilot-api.{domain}"),
        _ => DEFAULT_GITHUB_COPILOT_API_BASE_URL.to_string(),
    }
}

fn normalize_enterprise_domain(domain: String) -> String {
    domain
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug)]
pub struct GitHubCopilotTokenRefresher {
    codex_home: PathBuf,
    client: Client,
    refresh_lock: Semaphore,
}

impl GitHubCopilotTokenRefresher {
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            codex_home,
            client: build_github_copilot_client(),
            refresh_lock: Semaphore::new(/*permits*/ 1),
        }
    }

    pub fn with_client(codex_home: PathBuf, client: Client) -> Self {
        Self {
            codex_home,
            client,
            refresh_lock: Semaphore::new(/*permits*/ 1),
        }
    }

    async fn load_or_refresh(&self) -> std::io::Result<Option<GitHubCopilotAuth>> {
        let _permit =
            self.refresh_lock.acquire().await.map_err(|_| {
                std::io::Error::other("GitHub Copilot token refresh lock is closed.")
            })?;
        let Some(auth) = load_github_copilot_auth(&self.codex_home)? else {
            return Ok(None);
        };
        if is_copilot_token_stale(&auth) {
            let refreshed = refresh_github_copilot_auth(&self.client, auth).await?;
            save_github_copilot_auth(&self.codex_home, &refreshed)?;
            return Ok(Some(refreshed));
        }
        Ok(Some(auth))
    }

    async fn resolve(&self) -> std::io::Result<Option<ExternalAuthTokens>> {
        Ok(self
            .load_or_refresh()
            .await?
            .map(|auth| ExternalAuthTokens::access_token_only(auth.copilot_access_token)))
    }

    async fn refresh(
        &self,
        _context: ExternalAuthRefreshContext,
    ) -> std::io::Result<ExternalAuthTokens> {
        let _permit =
            self.refresh_lock.acquire().await.map_err(|_| {
                std::io::Error::other("GitHub Copilot token refresh lock is closed.")
            })?;
        let auth = load_github_copilot_auth(&self.codex_home)?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "GitHub Copilot auth is not configured. Run `codex login github-copilot`.",
            )
        })?;
        let refreshed = refresh_github_copilot_auth(&self.client, auth).await?;
        save_github_copilot_auth(&self.codex_home, &refreshed)?;
        Ok(ExternalAuthTokens::access_token_only(
            refreshed.copilot_access_token,
        ))
    }
}

impl ExternalAuth for GitHubCopilotTokenRefresher {
    fn auth_mode(&self) -> AuthMode {
        AuthMode::ApiKey
    }

    fn resolve(&self) -> ExternalAuthFuture<'_, Option<ExternalAuthTokens>> {
        Box::pin(GitHubCopilotTokenRefresher::resolve(self))
    }

    fn refresh(
        &self,
        context: ExternalAuthRefreshContext,
    ) -> ExternalAuthFuture<'_, ExternalAuthTokens> {
        Box::pin(GitHubCopilotTokenRefresher::refresh(self, context))
    }
}

pub async fn load_or_refresh_github_copilot_auth(
    codex_home: &Path,
) -> std::io::Result<Option<GitHubCopilotAuth>> {
    let refresher = GitHubCopilotTokenRefresher::new(codex_home.to_path_buf());
    refresher.load_or_refresh().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_proxy_endpoint_from_token() {
        assert_eq!(
            api_base_url_from_token("tid=1;proxy-ep=https%3A%2F%2Fcopilot.example.com;sku=x"),
            Some("https://copilot.example.com".to_string())
        );
        assert_eq!(
            api_base_url_from_token("tid=1;proxy-ep=proxy.individual.githubcopilot.com;sku=x"),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
        assert_eq!(
            api_base_url_from_token("tid=1;proxy-ep=api.enterprise.githubcopilot.com;sku=x"),
            Some("https://api.enterprise.githubcopilot.com".to_string())
        );
    }

    #[test]
    fn enterprise_default_api_base_url_uses_copilot_api_subdomain() {
        assert_eq!(
            default_api_base_url(Some("github.example.com")),
            "https://copilot-api.github.example.com"
        );
    }

    #[test]
    fn normalizes_enterprise_domain() {
        assert_eq!(
            GitHubCopilotAuthConfig::new(
                DEFAULT_GITHUB_COPILOT_CLIENT_ID,
                Some("https://github.example.com/".to_string()),
            )
            .enterprise_domain,
            Some("github.example.com".to_string())
        );
    }

    #[test]
    fn stale_tokens_include_refresh_skew() {
        let stale = GitHubCopilotAuth::new(
            "github-token".to_string(),
            "copilot-token".to_string(),
            Utc::now() + TimeDelta::seconds(TOKEN_REFRESH_SKEW_SECONDS - 1),
            DEFAULT_GITHUB_COPILOT_API_BASE_URL.to_string(),
            None,
        );
        let fresh = GitHubCopilotAuth::new(
            "github-token".to_string(),
            "copilot-token".to_string(),
            Utc::now() + TimeDelta::seconds(TOKEN_REFRESH_SKEW_SECONDS + 60),
            DEFAULT_GITHUB_COPILOT_API_BASE_URL.to_string(),
            None,
        );

        assert!(is_copilot_token_stale(&stale));
        assert!(!is_copilot_token_stale(&fresh));
    }

    #[test]
    fn enterprise_endpoints_use_api_subdomain() {
        let endpoints = GitHubCopilotEndpoints::for_enterprise_domain(Some("github.example.com"));

        assert_eq!(
            endpoints,
            GitHubCopilotEndpoints {
                device_code_url: "https://github.example.com/login/device/code".to_string(),
                access_token_url: "https://github.example.com/login/oauth/access_token".to_string(),
                copilot_token_url: "https://api.github.example.com/copilot_internal/v2/token"
                    .to_string(),
            }
        );
    }

    #[test]
    fn token_expiry_timestamp_is_representable() {
        let expires_at =
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("timestamp should parse");

        assert_eq!(
            expires_at,
            Utc.timestamp_opt(1_700_000_000, 0)
                .single()
                .expect("timestamp should parse")
        );
    }
}
