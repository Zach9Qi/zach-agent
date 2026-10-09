//! Messages 响应元数据、用量和结束原因转换。

use serde_json::Value;
use zach_ai_core::{
    FinishReason, GenerateResult, InputTokenUsage, ModelError, OutputTokenUsage, ProviderMetadata,
    ResponseMetadata, StreamAccumulator, StreamPart, UnifiedFinishReason, Usage,
};

use super::stream::MessagesStreamParser;

/// 非流式响应复用流式状态机：完整消息被视为一次性到达的全部事件。
pub(super) fn parse_response(value: Value) -> Result<GenerateResult, ModelError> {
    let mut parser = MessagesStreamParser::new(false);
    let parts = parser.complete(&value)?;
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        if let StreamPart::Error { message, raw } = part {
            return Err(ModelError::provider_error("anthropic", message, raw));
        }
        accumulator.process(part);
    }
    Ok(accumulator.finish())
}

pub(super) fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ModelError> {
    value.get(field).and_then(Value::as_str).ok_or_else(|| {
        ModelError::provider_error(
            "anthropic",
            format!("Messages 响应缺少字符串字段 {field}"),
            None,
        )
    })
}

pub(super) fn error(message: impl Into<String>, raw: Option<Value>) -> ModelError {
    ModelError::provider_error("anthropic", message, raw)
}

pub(super) fn metadata(value: Value) -> Option<ProviderMetadata> {
    let mut metadata = ProviderMetadata::new();
    metadata.insert("anthropic", value);
    Some(metadata)
}

pub(super) fn response_metadata(message: &Value) -> ResponseMetadata {
    ResponseMetadata {
        id: message["id"].as_str().map(str::to_owned),
        model_id: message["model"].as_str().map(str::to_owned),
        // Messages API 不返回创建时间戳。
        timestamp: None,
    }
}

/// Anthropic 的 `input_tokens` 不含缓存命中与写入部分，总量需三者相加。
pub(super) fn usage(value: &Value) -> Usage {
    let no_cache = value["input_tokens"].as_u64();
    let cache_read = value["cache_read_input_tokens"].as_u64();
    let cache_write = value["cache_creation_input_tokens"].as_u64();
    let total = if no_cache.is_some() || cache_read.is_some() || cache_write.is_some() {
        Some(no_cache.unwrap_or(0) + cache_read.unwrap_or(0) + cache_write.unwrap_or(0))
    } else {
        None
    };
    Usage {
        input_tokens: InputTokenUsage {
            total,
            no_cache,
            cache_read,
            cache_write,
        },
        output_tokens: OutputTokenUsage {
            total: value["output_tokens"].as_u64(),
            text: None,
            // 思考 token 计入 output_tokens，厂商不单独报告。
            reasoning: None,
        },
        raw: value.is_object().then(|| value.clone()),
    }
}

pub(super) fn finish_reason(stop_reason: &str) -> FinishReason {
    let unified = match stop_reason {
        "end_turn" | "stop_sequence" => UnifiedFinishReason::Stop,
        "max_tokens" | "model_context_window_exceeded" => UnifiedFinishReason::Length,
        "tool_use" => UnifiedFinishReason::ToolCalls,
        "refusal" => UnifiedFinishReason::ContentFilter,
        _ => UnifiedFinishReason::Other,
    };
    FinishReason {
        unified,
        raw: Some(stop_reason.to_owned()),
    }
}
