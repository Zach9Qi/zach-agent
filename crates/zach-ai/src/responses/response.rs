//! Responses 响应元数据、用量和完成原因转换。

use serde_json::{json, Value};
use zach_ai_core::{
    FinishReason, GenerateResult, InputTokenUsage, ModelError, OutputTokenUsage, ProviderMetadata,
    ResponseMetadata, StreamAccumulator, StreamPart, UnifiedFinishReason, Usage,
};

use super::stream::ResponsesStreamParser;

pub(super) fn parse_response(value: Value) -> Result<GenerateResult, ModelError> {
    let mut parser = ResponsesStreamParser::new(false);
    let parts = parser.complete(&value)?;
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        if let StreamPart::Error { message, raw } = part {
            return Err(ModelError::provider_error("openai", message, raw));
        }
        accumulator.process(part);
    }
    Ok(accumulator.finish())
}

pub(super) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ModelError> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        ModelError::provider_error(
            "openai",
            format!("Responses 响应缺少字符串字段 {field}"),
            None,
        )
    })
}

pub(super) fn metadata(value: Value) -> Option<ProviderMetadata> {
    let mut metadata = ProviderMetadata::new();
    metadata.insert("openai", value);
    Some(metadata)
}

pub(super) fn response_metadata(value: &Value) -> ResponseMetadata {
    ResponseMetadata {
        id: value["id"].as_str().map(str::to_owned),
        model_id: value["model"].as_str().map(str::to_owned),
        timestamp: value["created_at"]
            .as_i64()
            .and_then(|seconds| seconds.checked_mul(1000)),
    }
}

pub(super) fn finish(value: &Value) -> Result<StreamPart, ModelError> {
    let status = string(value, "status")?;
    let reason = value
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str);
    let output = value["output"].as_array().ok_or_else(|| {
        ModelError::provider_error("openai", "Responses 响应缺少 output 数组", None)
    })?;
    let has_calls = output.iter().any(|item| item["type"] == "function_call");
    let unified = match status {
        "completed" if has_calls => UnifiedFinishReason::ToolCalls,
        "completed" => UnifiedFinishReason::Stop,
        "incomplete" => match reason {
            Some("max_output_tokens") => UnifiedFinishReason::Length,
            Some("content_filter") => UnifiedFinishReason::ContentFilter,
            _ => {
                return Err(ModelError::provider_error(
                    "openai",
                    "Responses 返回未知的不完整状态",
                    Some(value.clone()),
                ))
            }
        },
        "failed" | "cancelled" => UnifiedFinishReason::Error,
        _ => {
            return Err(ModelError::provider_error(
                "openai",
                format!("Responses 尚未完成: {status}"),
                Some(value.clone()),
            ))
        }
    };
    Ok(StreamPart::Finish {
        usage: usage(&value["usage"]),
        finish_reason: FinishReason {
            unified,
            raw: Some(reason.unwrap_or(status).to_owned()),
        },
        provider_metadata: metadata(json!({
            "response_id": value["id"],
            "status": status,
            "incomplete_details": value["incomplete_details"],
        })),
    })
}

fn usage(value: &Value) -> Usage {
    let input = value["input_tokens"].as_u64();
    let cached = value
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64);
    let output = value["output_tokens"].as_u64();
    let reasoning = value
        .pointer("/output_tokens_details/reasoning_tokens")
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
            // 总输出还可能包含非文本 token，不把差值武断计为正文。
            text: None,
            reasoning,
        },
        raw: (!value.is_null()).then(|| value.clone()),
    }
}
