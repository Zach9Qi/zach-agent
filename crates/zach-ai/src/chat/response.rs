//! Chat Completions 非流式响应转换。

use serde_json::{json, Value};
use zach_ai_core::{
    FinishReason, GenerateResult, InputTokenUsage, ModelError, OutputTokenUsage, ProviderMetadata,
    ResponseMetadata, StreamAccumulator, StreamPart, UnifiedFinishReason, Usage,
};

pub(super) fn parse_response(value: Value) -> Result<GenerateResult, ModelError> {
    let choices = value["choices"]
        .as_array()
        .ok_or_else(|| error("响应缺少 choices 数组", &value))?;
    let choice = choices
        .first()
        .ok_or_else(|| error("响应 choices 为空", &value))?;
    let message = choice["message"]
        .as_object()
        .ok_or_else(|| error("响应缺少 message", &value))?;
    let mut accumulator = StreamAccumulator::new();
    accumulator.process(StreamPart::StreamStart { warnings: vec![] });
    accumulator.process(StreamPart::ResponseMetadata(response_metadata(&value)));
    let text_id = format!("choice:{}", choice["index"].as_u64().unwrap_or(0));
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        accumulator.process(StreamPart::TextStart {
            id: text_id.clone(),
            provider_metadata: metadata(json!({"refusal": false})),
        });
        accumulator.process(StreamPart::TextDelta {
            id: text_id.clone(),
            delta: text.into(),
            provider_metadata: None,
        });
        accumulator.process(StreamPart::TextEnd {
            id: text_id,
            provider_metadata: None,
        });
    }
    if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
        let id = format!("{}/refusal", choice["index"].as_u64().unwrap_or(0));
        accumulator.process(StreamPart::TextStart {
            id: id.clone(),
            provider_metadata: metadata(json!({"refusal": true})),
        });
        accumulator.process(StreamPart::TextDelta {
            id: id.clone(),
            delta: refusal.into(),
            provider_metadata: None,
        });
        accumulator.process(StreamPart::TextEnd {
            id,
            provider_metadata: None,
        });
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let id = string(call, "/id")?;
            let function = call
                .get("function")
                .ok_or_else(|| error("工具调用缺少 function", call))?;
            let name = string(function, "/name")?;
            let arguments = string(function, "/arguments")?;
            accumulator.process(StreamPart::ToolInputStart {
                id: id.clone(),
                tool_name: name.clone(),
                provider_executed: false,
                dynamic: false,
                title: None,
                provider_metadata: None,
            });
            accumulator.process(StreamPart::ToolInputDelta {
                id: id.clone(),
                delta: arguments.clone(),
                provider_metadata: None,
            });
            accumulator.process(StreamPart::ToolInputEnd {
                id: id.clone(),
                provider_metadata: None,
            });
            accumulator.process(StreamPart::ToolCall {
                tool_call_id: id,
                tool_name: name,
                input: arguments,
                provider_executed: false,
                dynamic: false,
                provider_metadata: None,
            });
        }
    }
    accumulator.process(StreamPart::Finish {
        usage: usage(&value["usage"]),
        finish_reason: finish_reason(choice["finish_reason"].as_str()),
        provider_metadata: metadata(json!({"object": value["object"]})),
    });
    Ok(accumulator.finish())
}

pub(super) fn response_metadata(value: &Value) -> ResponseMetadata {
    ResponseMetadata {
        id: value["id"].as_str().map(str::to_owned),
        model_id: value["model"].as_str().map(str::to_owned),
        timestamp: value["created"].as_i64().and_then(|s| s.checked_mul(1000)),
    }
}

pub(super) fn metadata(value: Value) -> Option<ProviderMetadata> {
    let mut result = ProviderMetadata::new();
    result.insert("openai", value);
    Some(result)
}

pub(super) fn usage(value: &Value) -> Usage {
    let input = value["prompt_tokens"].as_u64();
    let cached = value
        .pointer("/prompt_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let output = value["completion_tokens"].as_u64();
    let reasoning = value
        .pointer("/completion_tokens_details/reasoning_tokens")
        .and_then(Value::as_u64);
    Usage {
        input_tokens: InputTokenUsage {
            total: input,
            no_cache: input
                .zip(cached)
                .map(|(total, cached)| total.saturating_sub(cached)),
            cache_read: cached,
            cache_write: None,
        },
        output_tokens: OutputTokenUsage {
            total: output,
            text: None,
            reasoning,
        },
        raw: (!value.is_null()).then(|| value.clone()),
    }
}

pub(super) fn finish_reason(value: Option<&str>) -> FinishReason {
    let unified = match value.unwrap_or("stop") {
        "stop" => UnifiedFinishReason::Stop,
        "length" => UnifiedFinishReason::Length,
        "tool_calls" | "function_call" => UnifiedFinishReason::ToolCalls,
        "content_filter" => UnifiedFinishReason::ContentFilter,
        _ => UnifiedFinishReason::Other,
    };
    FinishReason {
        unified,
        raw: value.map(str::to_owned),
    }
}

fn string(value: &Value, path: &str) -> Result<String, ModelError> {
    let target = if path.starts_with('/') {
        value.pointer(path)
    } else {
        value.get(path)
    };
    target
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| error(&format!("响应缺少字符串字段 {path}"), value))
}
fn error(message: &str, raw: &Value) -> ModelError {
    ModelError::provider_error("openai", message, Some(raw.clone()))
}
