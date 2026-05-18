use crate::client_common::Prompt;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

const CHAT_COMPLETIONS_MAX_TOOL_NAME_BYTES: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct ChatCompletionsRequest {
    pub(crate) body: Value,
    pub(crate) tool_names: ChatToolNameMap,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ChatToolNameMap {
    forward: HashMap<ToolNameKey, String>,
    reverse: HashMap<String, ToolNameKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ToolNameKey {
    namespace: Option<String>,
    name: String,
}

impl ChatToolNameMap {
    fn insert(&mut self, namespace: Option<String>, name: String) -> String {
        let key = ToolNameKey { namespace, name };
        if let Some(existing) = self.forward.get(&key) {
            return existing.clone();
        }

        let base = sanitize_tool_name(&match key.namespace.as_ref() {
            Some(namespace) => format!("{namespace}__{}", key.name),
            None => key.name.clone(),
        });
        let mut candidate = bounded_tool_name(&base, &key, /*collision*/ 0);
        let mut collision = 1;
        while self
            .reverse
            .get(&candidate)
            .is_some_and(|existing| existing != &key)
        {
            candidate = bounded_tool_name(&base, &key, collision);
            collision += 1;
        }
        self.forward.insert(key.clone(), candidate.clone());
        self.reverse.insert(candidate.clone(), key);
        candidate
    }

    pub(crate) fn flatten(&self, namespace: Option<&str>, name: &str) -> String {
        self.forward
            .get(&ToolNameKey {
                namespace: namespace.map(str::to_string),
                name: name.to_string(),
            })
            .cloned()
            .unwrap_or_else(|| {
                sanitize_tool_name(&match namespace {
                    Some(namespace) => format!("{namespace}__{name}"),
                    None => name.to_string(),
                })
            })
    }

    pub(crate) fn unflatten(&self, name: &str) -> (Option<String>, String) {
        self.reverse
            .get(name)
            .map(|key| (key.namespace.clone(), key.name.clone()))
            .unwrap_or_else(|| (None, name.to_string()))
    }
}

pub(crate) fn build_chat_completions_request(
    prompt: &Prompt,
    model_info: &ModelInfo,
) -> Result<ChatCompletionsRequest> {
    let mut tool_names = ChatToolNameMap::default();
    let tools = chat_tools(&prompt.tools, &mut tool_names, model_info)?;
    let mut messages = Vec::new();
    let instructions = prompt.base_instructions.text.trim();
    if !instructions.is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions,
        }));
    }

    for item in prompt.get_formatted_input() {
        append_chat_message(&mut messages, &tool_names, item)?;
    }

    let mut body = json!({
        "model": model_info.slug,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_tokens": 4096,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = Value::String("auto".to_string());
        if prompt.parallel_tool_calls {
            body["parallel_tool_calls"] = Value::Bool(true);
        }
    }
    Ok(ChatCompletionsRequest { body, tool_names })
}

fn append_chat_message(
    messages: &mut Vec<Value>,
    tool_names: &ChatToolNameMap,
    item: ResponseItem,
) -> Result<()> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            if let Some(content) = chat_message_content(&role, content) {
                messages.push(json!({
                    "role": role,
                    "content": content,
                }));
            }
        }
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => {
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool_names.flatten(namespace.as_deref(), &name),
                        "arguments": arguments,
                    },
                }],
            }));
        }
        ResponseItem::CustomToolCall {
            call_id,
            name,
            input,
            ..
        } => {
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool_names.flatten(None, &name),
                        "arguments": input,
                    },
                }],
            }));
        }
        ResponseItem::FunctionCallOutput { call_id, output }
        | ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": function_output_to_text(&output.body),
            }));
        }
        ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => {}
    }
    Ok(())
}

fn chat_message_content(role: &str, content: Vec<ContentItem>) -> Option<Value> {
    let has_image = content
        .iter()
        .any(|item| matches!(item, ContentItem::InputImage { .. }));
    if has_image && role == "user" {
        let items = chat_content_items(content);
        return (!items.is_empty()).then_some(Value::Array(items));
    }

    let text = content_items_to_text(content);
    (!text.is_empty()).then_some(Value::String(text))
}

fn chat_content_items(content: Vec<ContentItem>) -> Vec<Value> {
    content
        .into_iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                (!text.is_empty()).then_some(json!({
                    "type": "text",
                    "text": text,
                }))
            }
            ContentItem::InputImage { image_url, .. } => Some(json!({
                "type": "image_url",
                "image_url": {"url": image_url},
            })),
        })
        .collect()
}

fn content_items_to_text(content: Vec<ContentItem>) -> String {
    content
        .into_iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                (!text.trim().is_empty()).then_some(text)
            }
            ContentItem::InputImage { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn function_output_to_text(output: &FunctionCallOutputBody) -> String {
    output
        .to_text()
        .unwrap_or_else(|| serde_json::to_string(output).unwrap_or_default())
}

fn chat_tools(
    tools: &[ToolSpec],
    tool_names: &mut ChatToolNameMap,
    _model_info: &ModelInfo,
) -> Result<Vec<Value>> {
    let mut chat_tools = Vec::new();
    for tool in tools {
        match tool {
            ToolSpec::Function(tool) => {
                chat_tools.push(chat_function_tool(
                    tool,
                    tool_names.insert(None, tool.name.clone()),
                )?);
            }
            ToolSpec::Namespace(namespace) => {
                for tool in &namespace.tools {
                    match tool {
                        ResponsesApiNamespaceTool::Function(tool) => {
                            chat_tools.push(chat_function_tool(
                                tool,
                                tool_names.insert(Some(namespace.name.clone()), tool.name.clone()),
                            )?);
                        }
                    }
                }
            }
            ToolSpec::WebSearch { .. } => {}
            ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::Freeform(_) => {
                return Err(CodexErr::InvalidRequest(format!(
                    "{} is not supported by GitHub Copilot Chat Completions models yet",
                    tool.name()
                )));
            }
        }
    }
    Ok(chat_tools)
}

fn chat_function_tool(tool: &ResponsesApiTool, name: String) -> serde_json::Result<Value> {
    Ok(json!({
        "type": "function",
        "function": {
            "name": name,
            "description": tool.description,
            "parameters": serde_json::to_value(&tool.parameters)?,
        },
    }))
}

fn sanitize_tool_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "tool".to_string()
    } else {
        sanitized
    }
}

fn bounded_tool_name(base: &str, key: &ToolNameKey, collision: u64) -> String {
    let suffix = if base.len() > CHAT_COMPLETIONS_MAX_TOOL_NAME_BYTES || collision > 0 {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        collision.hash(&mut hasher);
        Some(format!("{:08x}", hasher.finish() as u32))
    } else {
        None
    };
    let Some(suffix) = suffix else {
        return base.to_string();
    };
    let keep = CHAT_COMPLETIONS_MAX_TOOL_NAME_BYTES
        .saturating_sub(suffix.len())
        .saturating_sub(1);
    let mut prefix = base.chars().take(keep).collect::<String>();
    while prefix.ends_with('_') || prefix.ends_with('-') {
        prefix.pop();
    }
    if prefix.is_empty() {
        format!("tool_{suffix}")
    } else {
        format!("{prefix}_{suffix}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::models::FunctionCallOutputPayload;
    use codex_protocol::openai_models::ModelInfo;
    use codex_tools::JsonSchema;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    fn test_model_info() -> ModelInfo {
        serde_json::from_value(json!({
            "slug": "claude-test",
            "display_name": "Claude Test",
            "description": "desc",
            "supported_reasoning_levels": [],
            "shell_type": "shell_command",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 1,
            "upgrade": null,
            "base_instructions": "base",
            "model_messages": null,
            "supports_reasoning_summaries": false,
            "default_reasoning_summary": "auto",
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "experimental_supported_tools": [],
            "input_modalities": ["text"],
            "supports_search_tool": false
        }))
        .expect("model info should deserialize")
    }

    #[test]
    fn namespace_tool_names_round_trip_through_flattened_name() {
        let mut names = ChatToolNameMap::default();
        let flattened = names.insert(Some("mcp-server".to_string()), "read_file".to_string());

        assert_eq!(flattened, "mcp-server__read_file");
        assert_eq!(
            names.unflatten(&flattened),
            (Some("mcp-server".to_string()), "read_file".to_string())
        );
    }

    #[test]
    fn long_namespace_tool_names_are_bounded_and_reversible() {
        let mut names = ChatToolNameMap::default();
        let flattened = names.insert(
            Some("very-long-namespace-name-that-would-overflow".to_string()),
            "very-long-tool-name-that-would-also-overflow".to_string(),
        );

        assert!(flattened.len() <= CHAT_COMPLETIONS_MAX_TOOL_NAME_BYTES);
        assert_eq!(
            names.unflatten(&flattened),
            (
                Some("very-long-namespace-name-that-would-overflow".to_string()),
                "very-long-tool-name-that-would-also-overflow".to_string()
            )
        );
    }

    #[test]
    fn function_tools_are_mapped_to_chat_function_tools() {
        let mut names = ChatToolNameMap::default();
        let tools = chat_tools(
            &[ToolSpec::Function(ResponsesApiTool {
                name: "shell_command".to_string(),
                description: "Run a command".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::default(),
                output_schema: None,
            })],
            &mut names,
            &test_model_info(),
        )
        .expect("tool should map");

        assert_eq!(
            tools,
            vec![json!({
                "type": "function",
                "function": {
                    "name": "shell_command",
                    "description": "Run a command",
                    "parameters": {},
                }
            })]
        );
    }

    #[test]
    fn unsupported_hosted_web_search_tool_is_omitted_for_copilot_chat_models() {
        let mut names = ChatToolNameMap::default();
        let tools = chat_tools(
            &[ToolSpec::WebSearch {
                external_web_access: Some(true),
                filters: None,
                user_location: None,
                search_context_size: None,
                search_content_types: None,
            }],
            &mut names,
            &test_model_info(),
        )
        .expect("web search should be omitted");

        assert_eq!(tools, Vec::<Value>::new());
    }

    #[test]
    fn prompt_mapping_replays_tool_calls_and_outputs_with_same_ids() {
        let prompt = Prompt {
            input: vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "run pwd".to_string(),
                    }],
                    phase: None,
                },
                ResponseItem::FunctionCall {
                    id: Some("call_1".to_string()),
                    name: "shell_command".to_string(),
                    namespace: None,
                    arguments: "{\"command\":\"pwd\"}".to_string(),
                    call_id: "call_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("C:\\repo".to_string()),
                },
            ],
            tools: vec![],
            parallel_tool_calls: false,
            base_instructions: BaseInstructions {
                text: "You are helpful".to_string(),
            },
            personality: None,
            output_schema: None,
            output_schema_strict: true,
        };

        let request = build_chat_completions_request(&prompt, &test_model_info())
            .expect("request should build");

        assert_eq!(
            request.body["messages"],
            json!([
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "run pwd"},
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "shell_command",
                            "arguments": "{\"command\":\"pwd\"}"
                        }
                    }]
                },
                {"role": "tool", "tool_call_id": "call_1", "content": "C:\\repo"}
            ])
        );
    }
}
