use std::path::PathBuf;
use std::sync::Arc;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::default_client::build_default_reqwest_client_for_route_async;
use codex_login::github_copilot::GitHubCopilotTokenRefresher;
use codex_login::github_copilot::load_or_refresh_github_copilot_auth;
use codex_login::github_copilot_storage::load_github_copilot_auth;
use codex_model_provider_info::GITHUB_COPILOT_PROVIDER_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::manager::ModelsEndpointClient;
use codex_models_manager::manager::ModelsEndpointFuture;
use codex_models_manager::manager::OpenAiModelsManager;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_protocol::account::ProviderAccount;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use serde::Deserialize;
use serde_json::Value;
use std::str::FromStr;

use crate::auth::resolve_provider_auth;
use crate::provider::ModelProvider;
use crate::provider::ModelProviderFuture;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;

#[derive(Clone, Debug)]
pub(crate) struct GitHubCopilotModelProvider {
    info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl GitHubCopilotModelProvider {
    pub(crate) fn new(
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        let auth_manager = auth_manager.map(|base_manager| {
            let codex_home = base_manager.codex_home().to_path_buf();
            AuthManager::external_auth_only(
                codex_home.clone(),
                Arc::new(GitHubCopilotTokenRefresher::new(codex_home)),
            )
        });
        Self {
            info: provider_info,
            auth_manager,
        }
    }

    async fn token_base_url(&self) -> Option<String> {
        let codex_home = self.auth_manager.as_ref()?.codex_home().to_path_buf();
        load_or_refresh_github_copilot_auth(&codex_home)
            .await
            .ok()
            .flatten()
            .map(|auth| auth.api_base_url)
    }

    async fn auth(&self) -> Option<CodexAuth> {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await,
            None => None,
        }
    }

    async fn api_auth(&self) -> CoreResult<SharedAuthProvider> {
        let Some(auth) = self.auth().await else {
            return Err(CodexErr::InvalidRequest(
                "GitHub Copilot auth is not configured. Run `codex login github-copilot`."
                    .to_string(),
            ));
        };
        resolve_provider_auth(Some(&auth), &self.info)
    }

    async fn api_provider(&self) -> CoreResult<codex_api::Provider> {
        let auth = self.auth().await;
        let mut provider = self
            .info()
            .to_api_provider(auth.as_ref().map(CodexAuth::auth_mode))?;
        if let Some(base_url) = self.token_base_url().await {
            provider.base_url = base_url;
        }
        Ok(provider)
    }

    async fn runtime_base_url(&self) -> CoreResult<Option<String>> {
        Ok(self
            .token_base_url()
            .await
            .or_else(|| self.info.base_url.clone()))
    }
}

impl ModelProvider for GitHubCopilotModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            namespace_tools: true,
            image_generation: false,
            web_search: true,
        }
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(GitHubCopilotModelProvider::auth(self))
    }

    fn api_auth(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(GitHubCopilotModelProvider::api_auth(self))
    }

    fn account_state(&self) -> ProviderAccountResult {
        let account = self
            .auth_manager
            .as_ref()
            .and_then(|auth_manager| load_github_copilot_auth(auth_manager.codex_home()).ok())
            .flatten()
            .map(|_| ProviderAccount::GitHubCopilot);
        Ok(ProviderAccountState {
            account,
            requires_openai_auth: false,
        })
    }

    fn api_provider(&self) -> ModelProviderFuture<'_, codex_protocol::error::Result<Provider>> {
        Box::pin(GitHubCopilotModelProvider::api_provider(self))
    }

    fn runtime_base_url(
        &self,
    ) -> ModelProviderFuture<'_, codex_protocol::error::Result<Option<String>>> {
        Box::pin(GitHubCopilotModelProvider::runtime_base_url(self))
    }

    fn models_manager(
        &self,
        codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        match config_model_catalog {
            Some(model_catalog) => Arc::new(StaticModelsManager::new(
                self.auth_manager.clone(),
                model_catalog,
            )),
            None => {
                let endpoint = Arc::new(GitHubCopilotModelsEndpoint::new(
                    self.info.clone(),
                    self.auth_manager.clone(),
                ));
                Arc::new(OpenAiModelsManager::new_authoritative(
                    codex_home,
                    GITHUB_COPILOT_PROVIDER_ID,
                    endpoint,
                    self.auth_manager.clone(),
                ))
            }
        }
    }
}

#[derive(Debug)]
struct GitHubCopilotModelsEndpoint {
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl GitHubCopilotModelsEndpoint {
    fn new(provider_info: ModelProviderInfo, auth_manager: Option<Arc<AuthManager>>) -> Self {
        Self {
            provider_info,
            auth_manager,
        }
    }

    async fn auth(&self) -> Option<CodexAuth> {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await,
            None => None,
        }
    }

    async fn base_url(&self) -> String {
        if let Some(auth_manager) = self.auth_manager.as_ref()
            && let Ok(Some(auth)) =
                load_or_refresh_github_copilot_auth(auth_manager.codex_home()).await
        {
            return auth.api_base_url;
        }
        self.provider_info.base_url.clone().unwrap_or_else(|| {
            codex_model_provider_info::GITHUB_COPILOT_DEFAULT_BASE_URL.to_string()
        })
    }

    async fn uses_codex_backend(&self) -> bool {
        false
    }

    async fn list_models(
        &self,
        _client_version: &str,
        http_client_factory: HttpClientFactory,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        let Some(auth) = self.auth().await else {
            return Err(CodexErr::InvalidRequest(
                "GitHub Copilot auth is not configured. Run `codex login github-copilot`."
                    .to_string(),
            ));
        };

        let api_auth = resolve_provider_auth(Some(&auth), &self.provider_info)?;
        let mut headers = self
            .provider_info
            .to_api_provider(Some(auth.auth_mode()))?
            .headers;
        api_auth.add_auth_headers(&mut headers);
        let request_url = format!("{}/models", self.base_url().await.trim_end_matches('/'));
        let client = build_default_reqwest_client_for_route_async(
            http_client_factory,
            request_url.clone(),
            ClientRouteClass::Api,
        )
        .await?;
        let response = client
            .get(request_url)
            .headers(headers)
            .send()
            .await
            .map_err(std::io::Error::other)?;

        let status = response.status();
        let etag = response
            .headers()
            .get(http::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response.text().await.map_err(std::io::Error::other)?;
        if !status.is_success() {
            return Err(CodexErr::Io(std::io::Error::other(format!(
                "GitHub Copilot models request failed: {status}: {body}"
            ))));
        }

        let envelope: CopilotModelsEnvelope = serde_json::from_str(&body)?;
        let models = envelope
            .models()
            .into_iter()
            .enumerate()
            .filter_map(|(index, model)| model.into_model_info(index as i32))
            .collect();
        Ok((models, etag))
    }
}

impl ModelsEndpointClient for GitHubCopilotModelsEndpoint {
    fn has_command_auth(&self) -> bool {
        false
    }

    fn uses_codex_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(GitHubCopilotModelsEndpoint::uses_codex_backend(self))
    }

    fn list_models<'a>(
        &'a self,
        client_version: &'a str,
        http_client_factory: HttpClientFactory,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(GitHubCopilotModelsEndpoint::list_models(
            self,
            client_version,
            http_client_factory,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct CopilotModelsEnvelope {
    #[serde(default)]
    data: Vec<CopilotModel>,
    #[serde(default)]
    models: Vec<CopilotModel>,
}

impl CopilotModelsEnvelope {
    fn models(self) -> Vec<CopilotModel> {
        if self.data.is_empty() {
            self.models
        } else {
            self.data
        }
    }
}

#[derive(Debug, Deserialize)]
struct CopilotModel {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    owned_by: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    model_picker_enabled: Option<bool>,
    #[serde(default)]
    policy: Option<Value>,
    #[serde(default)]
    capabilities: Option<Value>,
    #[serde(default)]
    supported_endpoints: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopilotWireApiHint {
    Responses,
    ChatCompletions,
}

impl CopilotModel {
    fn into_model_info(self, priority: i32) -> Option<ModelInfo> {
        if !self.model_picker_enabled.unwrap_or(true) {
            return None;
        }
        if !policy_allows(self.policy.as_ref()) || !is_chat_capable(&self) {
            return None;
        }

        let display_name = self
            .name
            .clone()
            .or(self.version.clone())
            .unwrap_or_else(|| self.id.clone());
        let mut info = codex_models_manager::model_info::model_info_from_slug(&self.id);
        info.display_name = display_name.clone();
        let vendor = self.vendor();
        info.description = vendor
            .as_deref()
            .map(|vendor| format!("{display_name} via GitHub Copilot ({vendor})"))
            .or_else(|| Some(format!("{display_name} via GitHub Copilot")));
        info.supported_reasoning_levels = reasoning_efforts(self.capabilities.as_ref());
        info.default_reasoning_level = default_reasoning_effort(&info.supported_reasoning_levels);
        info.shell_type = ConfigShellToolType::ShellCommand;
        info.visibility = ModelVisibility::List;
        info.supported_in_api = true;
        info.priority = priority;
        info.upgrade = None;
        info.supports_reasoning_summaries = false;
        info.apply_patch_tool_type = None;
        let supports_web_search = supports_web_search(self.capabilities.as_ref());
        let supported_endpoints = self.supported_endpoints.as_deref();
        let wire_api_hint = copilot_wire_api_hint_from_supported_endpoints(supported_endpoints);
        let supports_responses_websocket = wire_api_hint == Some(CopilotWireApiHint::Responses)
            && copilot_supports_responses_websocket(supported_endpoints);
        info.experimental_supported_tools = copilot_metadata_markers(
            vendor.as_deref(),
            supports_web_search,
            wire_api_hint,
            supports_responses_websocket,
        );
        info.web_search_tool_type = WebSearchToolType::Text;
        info.supports_parallel_tool_calls = capability_bool(
            self.capabilities.as_ref(),
            &[
                &["supports_parallel_tool_calls"],
                &["supports", "parallel_tool_calls"],
                &["supports", "parallelToolCalls"],
            ],
        );
        let supports_vision = capability_bool(
            self.capabilities.as_ref(),
            &[
                &["supports_vision"],
                &["supports", "vision"],
                &["supports", "image"],
            ],
        ) || modality_contains(self.capabilities.as_ref(), "image");
        info.supports_image_detail_original = supports_vision;
        info.input_modalities = if supports_vision {
            vec![InputModality::Text, InputModality::Image]
        } else {
            vec![InputModality::Text]
        };
        if let Some(context_window) = context_window_tokens(self.capabilities.as_ref()) {
            info.context_window = Some(context_window);
            info.max_context_window = Some(context_window);
            info.truncation_policy = TruncationPolicyConfig::tokens(context_window);
        }

        Some(info)
    }

    fn vendor(&self) -> Option<String> {
        let candidates = [
            self.vendor.as_deref(),
            self.owned_by.as_deref(),
            self.provider.as_deref(),
            self.publisher.as_deref(),
        ];

        if let Some(vendor) = candidates
            .iter()
            .flatten()
            .copied()
            .find(|vendor| !is_openai_copilot_vendor(vendor))
        {
            return Some(vendor.to_string());
        }

        non_openai_vendor_from_model_id(&self.id)
            .or_else(|| candidates.into_iter().flatten().next().map(str::to_string))
    }
}

fn is_openai_copilot_vendor(vendor: &str) -> bool {
    matches!(
        normalize_copilot_vendor(vendor).as_deref(),
        Some("openai" | "open-ai" | "azureopenai" | "azure-openai" | "azure-open-ai")
    )
}

fn non_openai_vendor_from_model_id(model_id: &str) -> Option<String> {
    let family = model_family_from_id(model_id)?;
    match family.as_str() {
        "claude" => Some("Anthropic".to_string()),
        "gemini" => Some("Google".to_string()),
        "mistral" => Some("mistral".to_string()),
        _ => None,
    }
}

fn model_family_from_id(model_id: &str) -> Option<String> {
    let normalized = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = normalized
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(normalized.as_str());
    let family = model.split('-').next()?;
    (!family.is_empty()).then(|| family.to_string())
}

fn copilot_metadata_markers(
    vendor: Option<&str>,
    supports_web_search: bool,
    wire_api_hint: Option<CopilotWireApiHint>,
    supports_responses_websocket: bool,
) -> Vec<String> {
    let mut markers = Vec::new();
    if let Some(vendor) = vendor.and_then(normalize_copilot_vendor) {
        markers.push(format!("github_copilot_vendor:{vendor}"));
    }
    match wire_api_hint {
        Some(CopilotWireApiHint::Responses) => {
            markers.push("github_copilot_wire_api:responses".to_string());
        }
        Some(CopilotWireApiHint::ChatCompletions) => {
            markers.push("github_copilot_wire_api:chat-completions".to_string());
        }
        None => {}
    }
    if supports_web_search {
        markers.push("github_copilot_web_search".to_string());
    }
    if supports_responses_websocket {
        markers.push("github_copilot_responses_websocket".to_string());
    }
    markers
}

fn copilot_wire_api_hint_from_supported_endpoints(
    supported_endpoints: Option<&[String]>,
) -> Option<CopilotWireApiHint> {
    let supported_endpoints = supported_endpoints?;
    if supported_endpoints.is_empty() {
        return None;
    }
    if supported_endpoints
        .iter()
        .any(|endpoint| matches!(endpoint.trim().to_ascii_lowercase().as_str(), "/responses"))
    {
        return Some(CopilotWireApiHint::Responses);
    }
    Some(CopilotWireApiHint::ChatCompletions)
}

fn copilot_supports_responses_websocket(supported_endpoints: Option<&[String]>) -> bool {
    supported_endpoints.is_some_and(|supported_endpoints| {
        supported_endpoints
            .iter()
            .any(|endpoint| endpoint.trim().eq_ignore_ascii_case("ws:/responses"))
    })
}

fn normalize_copilot_vendor(vendor: &str) -> Option<String> {
    let normalized = vendor.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    (!normalized.is_empty()).then_some(normalized)
}

fn supports_web_search(capabilities: Option<&Value>) -> bool {
    let Some(capabilities) = capabilities else {
        return true;
    };
    if capability_bool(
        Some(capabilities),
        &[
            &["supports_web_search"],
            &["supportsWebSearch"],
            &["web_search"],
            &["webSearch"],
            &["supports", "web_search"],
            &["supports", "webSearch"],
            &["supports", "search"],
        ],
    ) {
        return true;
    }
    !capability_bool(
        Some(capabilities),
        &[
            &["supports", "web_search_disabled"],
            &["supports", "webSearchDisabled"],
            &["web_search_disabled"],
            &["webSearchDisabled"],
        ],
    )
}

fn policy_allows(policy: Option<&Value>) -> bool {
    let Some(policy) = policy else {
        return true;
    };
    match policy {
        Value::Bool(value) => *value,
        Value::String(value) => !is_disabled_policy_value(value),
        Value::Object(map) => {
            if let Some(enabled) = map.get("enabled").and_then(Value::as_bool)
                && !enabled
            {
                return false;
            }
            !["state", "status", "result"].iter().any(|key| {
                map.get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(is_disabled_policy_value)
            })
        }
        _ => true,
    }
}

fn is_disabled_policy_value(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "disabled" | "blocked" | "denied" | "unavailable" | "off"
    )
}

fn is_chat_capable(model: &CopilotModel) -> bool {
    if model.id.to_ascii_lowercase().contains("embedding") {
        return false;
    }
    let Some(capabilities) = model.capabilities.as_ref() else {
        return true;
    };
    if value_contains_any(capabilities, &["embedding", "embeddings"]) {
        return false;
    }
    if let Some(kind) = first_string_at_any(capabilities, &[&["type"], &["kind"], &["family"]]) {
        return !matches!(
            kind.to_ascii_lowercase().as_str(),
            "embedding" | "embeddings" | "completion" | "completions"
        );
    }
    true
}

fn context_window_tokens(capabilities: Option<&Value>) -> Option<i64> {
    let capabilities = capabilities?;
    first_i64_at_any(
        capabilities,
        &[
            &["limits", "max_context_window_tokens"],
            &["limits", "maxContextWindowTokens"],
            &["limits", "max_prompt_tokens"],
            &["max_context_window_tokens"],
            &["maxContextWindowTokens"],
            &["context_window"],
            &["contextWindow"],
        ],
    )
}

fn capability_bool(capabilities: Option<&Value>, paths: &[&[&str]]) -> bool {
    let Some(capabilities) = capabilities else {
        return false;
    };
    paths.iter().any(|path| {
        value_at_path(capabilities, path)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
}

fn modality_contains(capabilities: Option<&Value>, modality: &str) -> bool {
    let Some(capabilities) = capabilities else {
        return false;
    };
    [
        &["input_modalities"][..],
        &["inputModalities"][..],
        &["modalities"][..],
    ]
    .iter()
    .filter_map(|path| value_at_path(capabilities, path))
    .any(|value| value_contains_any(value, &[modality]))
}

fn reasoning_efforts(capabilities: Option<&Value>) -> Vec<ReasoningEffortPreset> {
    let Some(value) = capabilities
        .and_then(|capabilities| value_at_path(capabilities, &["supports", "reasoning_effort"]))
    else {
        return Vec::new();
    };

    let Some(items) = value.as_array() else {
        return Vec::new();
    };

    let mut efforts = Vec::new();
    for item in items {
        let Some(value) = item.as_str() else {
            continue;
        };
        let Ok(effort) = ReasoningEffort::from_str(value) else {
            continue;
        };
        if efforts
            .iter()
            .any(|preset: &ReasoningEffortPreset| preset.effort == effort)
        {
            continue;
        }
        efforts.push(ReasoningEffortPreset {
            description: effort.to_string(),
            effort: effort.clone(),
        });
    }
    efforts
}

fn default_reasoning_effort(supported: &[ReasoningEffortPreset]) -> Option<ReasoningEffort> {
    if supported.is_empty() {
        return None;
    }
    supported
        .iter()
        .find(|preset| preset.effort == ReasoningEffort::Medium)
        .or_else(|| supported.first())
        .map(|preset| preset.effort.clone())
}
fn first_i64_at_any(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    paths
        .iter()
        .filter_map(|path| value_at_path(value, path))
        .find_map(serde_json::Value::as_i64)
}

fn first_string_at_any<'a>(value: &'a Value, paths: &[&[&str]]) -> Option<&'a str> {
    paths
        .iter()
        .filter_map(|path| value_at_path(value, path))
        .find_map(Value::as_str)
}

fn value_at_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn value_contains_any(value: &Value, expected: &[&str]) -> bool {
    match value {
        Value::String(value) => expected
            .iter()
            .any(|expected| value.eq_ignore_ascii_case(expected)),
        Value::Array(items) => items.iter().any(|item| value_contains_any(item, expected)),
        Value::Object(map) => map.values().any(|item| value_contains_any(item, expected)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn stored_auth(api_base_url: &str) -> codex_login::github_copilot_storage::GitHubCopilotAuth {
        serde_json::from_value(json!({
            "github_access_token": "github-token",
            "copilot_access_token": "copilot-token",
            "copilot_token_expires_at": "2030-01-01T00:00:00Z",
            "api_base_url": api_base_url,
            "saved_at": "2030-01-01T00:00:00Z",
        }))
        .expect("auth should deserialize")
    }

    fn test_codex_home(test_name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time should be after UNIX_EPOCH")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "codex-github-copilot-{test_name}-{}-{unique}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&path).expect("temp codex home should be created");
        path
    }

    #[test]
    fn maps_copilot_reasoning_efforts_from_capabilities() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "gpt-5.4",
            "name": "GPT-5.4",
            "vendor": "OpenAI",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"reasoning_effort": ["low", "medium", "high", "xhigh"]}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(info.default_reasoning_level, Some(ReasoningEffort::Medium));
        assert_eq!(
            info.supported_reasoning_levels,
            vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "low".to_string()
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "medium".to_string()
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "high".to_string()
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::XHigh,
                    description: "xhigh".to_string()
                },
            ]
        );
    }

    #[test]
    fn maps_single_copilot_reasoning_effort_as_default() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "claude-opus-4.7-high",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"reasoning_effort": ["high"]}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(info.default_reasoning_level, Some(ReasoningEffort::High));
        assert_eq!(info.supported_reasoning_levels.len(), 1);
    }

    #[test]
    fn accepts_unknown_copilot_reasoning_efforts_as_custom() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "gpt-5.4-mini",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"reasoning_effort": ["medium", "turbo"]}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(info.default_reasoning_level, Some(ReasoningEffort::Medium));
        assert_eq!(
            info.supported_reasoning_levels,
            vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Medium,
                    description: "medium".to_string()
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Custom("turbo".to_string()),
                    description: "turbo".to_string()
                }
            ]
        );
    }
    #[test]
    fn maps_selectable_copilot_chat_model() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "claude-sonnet-4.5",
            "name": "Claude Sonnet 4.5",
            "owned_by": "anthropic",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "limits": {"max_context_window_tokens": 200000},
                "supports": {"vision": true, "parallel_tool_calls": true}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 3)
            .expect("model should be selectable");

        assert_eq!(info.slug, "claude-sonnet-4.5");
        assert_eq!(info.display_name, "Claude Sonnet 4.5");
        assert_eq!(info.priority, 3);
        assert_eq!(info.context_window, Some(200_000));
        assert_eq!(
            info.experimental_supported_tools,
            vec![
                "github_copilot_vendor:anthropic".to_string(),
                "github_copilot_web_search".to_string()
            ]
        );
        assert_eq!(
            info.input_modalities,
            vec![InputModality::Text, InputModality::Image]
        );
        assert!(info.supports_parallel_tool_calls);
    }

    #[test]
    fn maps_copilot_vendor_from_alternate_owner_fields() {
        for owner_field in ["owned_by", "provider", "publisher"] {
            let model_id = format!("{owner_field}-model");
            let mut value = json!({
                "id": model_id,
                "model_picker_enabled": true,
                "policy": {"state": "enabled"},
                "capabilities": {"type": "chat"}
            });
            value
                .as_object_mut()
                .expect("model fixture should be an object")
                .insert(owner_field.to_string(), json!("Google"));
            let model: CopilotModel = serde_json::from_value(value).expect("model should parse");

            let info = model
                .into_model_info(/*priority*/ 0)
                .expect("model should be selectable");

            assert_eq!(
                info.experimental_supported_tools[0],
                "github_copilot_vendor:google"
            );
            let expected_description = format!("{owner_field}-model via GitHub Copilot (Google)");
            assert_eq!(
                info.description.as_deref(),
                Some(expected_description.as_str())
            );
        }
    }

    #[test]
    fn maps_non_openai_vendor_from_vendor_before_owner_fields() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "vendor-wins-model",
            "vendor": "Anthropic",
            "owned_by": "Google",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {"type": "chat"}
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools[0],
            "github_copilot_vendor:anthropic"
        );
        assert_eq!(
            info.description.as_deref(),
            Some("vendor-wins-model via GitHub Copilot (Anthropic)")
        );
    }

    #[test]
    fn maps_non_openai_owner_before_generic_openai_vendor() {
        for owner_field in ["owned_by", "provider", "publisher"] {
            let model_id = format!("generic-openai-{owner_field}-model");
            let mut value = json!({
                "id": model_id,
                "vendor": "OpenAI",
                "model_picker_enabled": true,
                "policy": {"state": "enabled"},
                "capabilities": {"type": "chat"}
            });
            value
                .as_object_mut()
                .expect("model fixture should be an object")
                .insert(owner_field.to_string(), json!("Google"));
            let model: CopilotModel = serde_json::from_value(value).expect("model should parse");

            let info = model
                .into_model_info(/*priority*/ 0)
                .expect("model should be selectable");

            assert_eq!(
                info.experimental_supported_tools[0],
                "github_copilot_vendor:google"
            );
            let expected_description =
                format!("generic-openai-{owner_field}-model via GitHub Copilot (Google)");
            assert_eq!(
                info.description.as_deref(),
                Some(expected_description.as_str())
            );
        }
    }

    #[test]
    fn maps_responses_supported_endpoint_to_wire_api_markers() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "lark-picker-secondary",
            "name": "Lark",
            "vendor": "Azure OpenAI",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            },
            "supported_endpoints": ["/responses", "ws:/responses"]
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec![
                "github_copilot_vendor:azure-openai".to_string(),
                "github_copilot_wire_api:responses".to_string(),
                "github_copilot_responses_websocket".to_string(),
            ]
        );
        assert_eq!(
            info.description.as_deref(),
            Some("Lark via GitHub Copilot (Azure OpenAI)")
        );
    }

    #[test]
    fn maps_non_responses_supported_endpoints_to_chat_completions_marker() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "claude-sonnet-4.5",
            "vendor": "Anthropic",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            },
            "supported_endpoints": ["/v1/messages", "/chat/completions"]
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec![
                "github_copilot_vendor:anthropic".to_string(),
                "github_copilot_wire_api:chat-completions".to_string(),
            ]
        );
    }

    #[test]
    fn maps_websocket_only_responses_endpoint_to_chat_completions_marker() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "future-ws-only-model",
            "vendor": "OpenAI",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            },
            "supported_endpoints": ["ws:/responses"]
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec![
                "github_copilot_vendor:openai".to_string(),
                "github_copilot_wire_api:chat-completions".to_string(),
            ]
        );
    }

    #[test]
    fn omits_websocket_marker_without_http_responses_endpoint() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "gpt-5.2-codex",
            "vendor": "OpenAI",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            },
            "supported_endpoints": ["/responses"]
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec![
                "github_copilot_vendor:openai".to_string(),
                "github_copilot_wire_api:responses".to_string(),
            ]
        );
    }

    #[test]
    fn omits_wire_api_marker_when_supported_endpoints_are_missing() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "gemini-2.5-pro",
            "vendor": "Google",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec!["github_copilot_vendor:google".to_string()]
        );
    }

    #[test]
    fn infers_non_openai_vendor_from_model_family_when_metadata_is_generic() {
        for (id, expected_vendor, expected_marker) in [
            (
                "gemini-3.1-pro-preview",
                "Google",
                "github_copilot_vendor:google",
            ),
            (
                "claude-sonnet-4.5",
                "Anthropic",
                "github_copilot_vendor:anthropic",
            ),
            (
                "mistral-large-latest",
                "mistral",
                "github_copilot_vendor:mistral",
            ),
        ] {
            let model: CopilotModel = serde_json::from_value(json!({
                "id": id,
                "vendor": "OpenAI",
                "model_picker_enabled": true,
                "policy": {"state": "enabled"},
                "capabilities": {"type": "chat"}
            }))
            .expect("model should parse");

            let info = model
                .into_model_info(/*priority*/ 0)
                .expect("model should be selectable");

            assert_eq!(info.experimental_supported_tools[0], expected_marker);
            assert_eq!(
                info.description.as_deref(),
                Some(format!("{id} via GitHub Copilot ({expected_vendor})").as_str())
            );
        }
    }

    #[test]
    fn does_not_infer_unknown_vendor_from_model_family_when_metadata_is_openai() {
        let model: CopilotModel = serde_json::from_value(json!({
            "id": "lark-picker-secondary",
            "vendor": "OpenAI",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {
                "type": "chat",
                "supports": {"web_search_disabled": true}
            }
        }))
        .expect("model should parse");

        let info = model
            .into_model_info(/*priority*/ 0)
            .expect("model should be selectable");

        assert_eq!(
            info.experimental_supported_tools,
            vec!["github_copilot_vendor:openai".to_string()]
        );
        assert_eq!(
            info.description.as_deref(),
            Some("lark-picker-secondary via GitHub Copilot (OpenAI)")
        );
    }

    #[test]
    fn filters_disabled_and_embedding_models() {
        let disabled: CopilotModel = serde_json::from_value(json!({
            "id": "gpt-disabled",
            "model_picker_enabled": true,
            "policy": {"state": "disabled"},
            "capabilities": {"type": "chat"}
        }))
        .expect("model should parse");
        let embedding: CopilotModel = serde_json::from_value(json!({
            "id": "text-embedding-3-large",
            "model_picker_enabled": true,
            "policy": {"state": "enabled"},
            "capabilities": {"type": "embedding"}
        }))
        .expect("model should parse");

        assert_eq!(disabled.into_model_info(/*priority*/ 0), None);
        assert_eq!(embedding.into_model_info(/*priority*/ 0), None);
    }

    #[test]
    fn supports_data_and_models_envelopes() {
        let data: CopilotModelsEnvelope = serde_json::from_value(json!({
            "data": [{"id": "gpt-4o"}],
            "models": [{"id": "ignored"}]
        }))
        .expect("envelope should parse");
        let models: CopilotModelsEnvelope = serde_json::from_value(json!({
            "models": [{"id": "fallback"}]
        }))
        .expect("envelope should parse");

        assert_eq!(data.models()[0].id, "gpt-4o");
        assert_eq!(models.models()[0].id, "fallback");
    }

    #[test]
    fn github_copilot_provider_reports_account_when_auth_file_exists() {
        let codex_home = test_codex_home("account-state");
        codex_login::github_copilot_storage::save_github_copilot_auth(
            &codex_home,
            &stored_auth(codex_login::github_copilot::DEFAULT_GITHUB_COPILOT_API_BASE_URL),
        )
        .expect("auth should save");
        let base_auth_manager = AuthManager::from_auth_for_testing_with_home(
            CodexAuth::from_api_key("unused"),
            codex_home,
        );
        let provider = GitHubCopilotModelProvider::new(
            ModelProviderInfo::create_github_copilot_provider(),
            Some(base_auth_manager),
        );

        assert_eq!(
            provider.account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::GitHubCopilot),
                requires_openai_auth: false,
            })
        );
    }

    #[tokio::test]
    async fn github_copilot_provider_uses_stored_token_for_requests() {
        let codex_home = test_codex_home("api-auth");
        codex_login::github_copilot_storage::save_github_copilot_auth(
            &codex_home,
            &stored_auth("https://copilot-proxy.example.com"),
        )
        .expect("auth should save");
        let base_auth_manager = AuthManager::from_auth_for_testing_with_home(
            CodexAuth::from_api_key("unused"),
            codex_home,
        );
        let provider = GitHubCopilotModelProvider::new(
            ModelProviderInfo::create_github_copilot_provider(),
            Some(base_auth_manager),
        );

        let api_provider = provider
            .api_provider()
            .await
            .expect("API provider should resolve");
        let auth_headers = provider
            .api_auth()
            .await
            .expect("API auth should resolve")
            .to_auth_headers();

        assert_eq!(api_provider.base_url, "https://copilot-proxy.example.com");
        assert_eq!(
            auth_headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer copilot-token")
        );
    }
}
