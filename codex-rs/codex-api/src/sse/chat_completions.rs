use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::rate_limits::parse_all_rate_limits;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const OPENAI_MODEL_HEADER: &str = "openai-model";
const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let server_model = stream_response
        .headers
        .get(OPENAI_MODEL_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        if let Some(model) = server_model {
            let _ = tx_event.send(Ok(ResponseEvent::ServerModel(model))).await;
        }
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_chat_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Default)]
struct ChatStreamState {
    response_id: Option<String>,
    message_started: bool,
    message_id: Option<String>,
    text: String,
    tool_calls: BTreeMap<usize, ChatToolCallState>,
    usage: Option<TokenUsage>,
    finished_items: bool,
}

#[derive(Debug, Default)]
struct ChatToolCallState {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    id: Option<String>,
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    delta: Option<ChatDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatDelta {
    content: Option<String>,
    refusal: Option<String>,
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
    prompt_tokens_details: Option<ChatPromptTokensDetails>,
    completion_tokens_details: Option<ChatCompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ChatPromptTokensDetails {
    cached_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionTokensDetails {
    reasoning_tokens: Option<i64>,
}

impl From<ChatUsage> for TokenUsage {
    fn from(value: ChatUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            cached_input_tokens: value
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens)
                .unwrap_or(0),
            output_tokens: value.completion_tokens,
            reasoning_output_tokens: value
                .completion_tokens_details
                .and_then(|details| details.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: value.total_tokens,
        }
    }
}

pub async fn process_chat_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = stream.eventsource();
    let mut state = ChatStreamState::default();

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("SSE Error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before chat completion finished".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("Chat completions SSE event: {}", &sse.data);

        if sse.data.trim() == "[DONE]" {
            finalize_chat_items(&mut state, &tx_event).await;
            let response_id = state
                .response_id
                .clone()
                .unwrap_or_else(|| "chatcmpl_unknown".to_string());
            let _ = tx_event
                .send(Ok(ResponseEvent::Completed {
                    response_id,
                    token_usage: state.usage.take(),
                    end_turn: Some(true),
                }))
                .await;
            return;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(e) => {
                debug!(
                    "Failed to parse chat completions SSE event: {e}, data: {}",
                    &sse.data
                );
                continue;
            }
        };

        if state.response_id.is_none() {
            state.response_id = chunk.id.clone();
            let _ = tx_event.send(Ok(ResponseEvent::Created)).await;
        }
        if let Some(usage) = chunk.usage {
            state.usage = Some(usage.into());
        }

        for choice in chunk.choices {
            if let Some(delta) = choice.delta {
                if let Some(content) = delta.content {
                    append_text_delta(&mut state, &tx_event, content).await;
                }
                if let Some(refusal) = delta.refusal {
                    append_text_delta(&mut state, &tx_event, refusal).await;
                }
                if let Some(tool_calls) = delta.tool_calls {
                    for tool_call in tool_calls {
                        let state = state.tool_calls.entry(tool_call.index).or_default();
                        if let Some(id) = tool_call.id {
                            state.id = Some(id);
                        }
                        if let Some(function) = tool_call.function {
                            if let Some(name) = function.name {
                                state.name = Some(name);
                            }
                            if let Some(arguments) = function.arguments {
                                state.arguments.push_str(&arguments);
                            }
                        }
                    }
                }
            }

            if let Some(finish_reason) = choice.finish_reason {
                match finish_reason.as_str() {
                    "stop" | "tool_calls" => {
                        finalize_chat_items(&mut state, &tx_event).await;
                    }
                    "length" => {
                        let _ = tx_event
                            .send(Err(ApiError::Stream(
                                "chat completion stopped because the output length limit was reached"
                                    .to_string(),
                            )))
                            .await;
                        return;
                    }
                    "content_filter" => {
                        let _ = tx_event
                            .send(Err(ApiError::InvalidRequest {
                                message: "chat completion was blocked by the content filter"
                                    .to_string(),
                            }))
                            .await;
                        return;
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn append_text_delta(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
    delta: String,
) {
    if delta.is_empty() {
        return;
    }
    if !state.message_started {
        let response_id = state
            .response_id
            .clone()
            .unwrap_or_else(|| "chatcmpl_unknown".to_string());
        let message_id = format!("{response_id}_message");
        state.message_id = Some(message_id);
        state.message_started = true;
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                id: state.message_id.clone(),
                role: "assistant".to_string(),
                content: Vec::new(),
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await;
    }
    state.text.push_str(&delta);
    let _ = tx_event
        .send(Ok(ResponseEvent::OutputTextDelta(delta)))
        .await;
}

async fn finalize_chat_items(
    state: &mut ChatStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) {
    if state.finished_items {
        return;
    }
    state.finished_items = true;

    if !state.text.is_empty() || state.message_started {
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: state.message_id.clone(),
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: state.text.clone(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })))
            .await;
    }

    for (index, tool_call) in &state.tool_calls {
        let name = tool_call
            .name
            .clone()
            .unwrap_or_else(|| format!("unknown_tool_{index}"));
        let call_id = tool_call
            .id
            .clone()
            .unwrap_or_else(|| format!("call_{index}"));
        let _ = tx_event
            .send(Ok(ResponseEvent::OutputItemDone(
                ResponseItem::FunctionCall {
                    id: Some(call_id.clone()),
                    name,
                    namespace: None,
                    arguments: tool_call.arguments.clone(),
                    call_id,
                    internal_chat_message_metadata_passthrough: None,
                },
            )))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_client::TransportError;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tokio::sync::mpsc;
    use tokio_test::io::Builder as IoBuilder;
    use tokio_util::io::ReaderStream;

    async fn collect_events(body: &str) -> Vec<Result<ResponseEvent, ApiError>> {
        let mut builder = IoBuilder::new();
        builder.read(body.as_bytes());
        let reader = builder.build();
        let stream =
            ReaderStream::new(reader).map_err(|err| TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        tokio::spawn(process_chat_sse(
            Box::pin(stream),
            tx,
            Duration::from_millis(1000),
            /*telemetry*/ None,
        ));

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn parses_text_usage_and_completed() {
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "id": "chatcmpl-1",
                "choices": [{"delta": {"content": "Hello"}}]
            }),
            json!({
                "id": "chatcmpl-1",
                "choices": [{"delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}
            })
        );

        let events = collect_events(&body).await;

        assert!(matches!(events[0], Ok(ResponseEvent::Created)));
        assert!(matches!(
            events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        ));
        assert!(matches!(
            &events[2],
            Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Hello"
        ));
        assert!(matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "Hello".to_string() }]
        ));
        assert!(matches!(
            &events[4],
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage: Some(usage),
                end_turn: Some(true),
            }) if response_id == "chatcmpl-1"
                && usage.input_tokens == 1
                && usage.output_tokens == 2
                && usage.total_tokens == 3
        ));
    }

    #[tokio::test]
    async fn accumulates_interleaved_parallel_tool_calls() {
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({
                "id": "chatcmpl-2",
                "choices": [{"delta": {"tool_calls": [
                    {"index": 0, "id": "call_a", "function": {"name": "tool_a", "arguments": "{\"a\""}},
                    {"index": 1, "id": "call_b", "function": {"name": "tool_b", "arguments": "{\"b\""}}
                ]}}]
            }),
            json!({
                "id": "chatcmpl-2",
                "choices": [{"delta": {"tool_calls": [
                    {"index": 1, "function": {"arguments": ":2}"}},
                    {"index": 0, "function": {"arguments": ":1}"}}
                ]}}]
            }),
            json!({
                "id": "chatcmpl-2",
                "choices": [{"delta": {}, "finish_reason": "tool_calls"}]
            })
        );

        let events = collect_events(&body).await;
        let tool_calls = events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                    name,
                    call_id,
                    arguments,
                    ..
                })) => Some((name.clone(), call_id.clone(), arguments.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            tool_calls,
            vec![
                (
                    "tool_a".to_string(),
                    "call_a".to_string(),
                    "{\"a\":1}".to_string()
                ),
                (
                    "tool_b".to_string(),
                    "call_b".to_string(),
                    "{\"b\":2}".to_string()
                ),
            ]
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                Ok(ResponseEvent::Completed {
                    response_id,
                    end_turn: Some(true),
                    ..
                }) if response_id == "chatcmpl-2"
            )
        }));
    }
}
