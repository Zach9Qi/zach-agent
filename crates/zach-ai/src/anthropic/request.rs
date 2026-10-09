//! Messages API 请求参数映射，消息回放与内容块转换分别由子模块负责。

mod blocks;
mod messages;
mod tools;

#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use zach_ai_core::{CallOptions, ModelError, ProviderOptions, ReasoningEffort, ResponseFormat};

/// 模型档案未知时使用的 `max_tokens` 默认值（Messages API 要求必填）。
const DEFAULT_MAX_TOKENS: u64 = 8192;

/// 已构建的请求：正文与请求派生的 `anthropic-beta` 标记。
#[derive(Debug)]
pub(super) struct BuiltRequest {
    pub(super) body: Value,
    pub(super) betas: Vec<String>,
}

pub(super) fn build_request(
    model_id: &str,
    options: &CallOptions,
    stream: bool,
) -> Result<BuiltRequest, ModelError> {
    if model_id.trim().is_empty() {
        return Err(ModelError::InvalidRequest("模型 ID 不能为空".into()));
    }
    reject_unsupported(options)?;
    let converted = messages::convert(&options.prompt.messages)?;
    let mut body = json!({
        "model": model_id,
        "max_tokens": options
            .max_output_tokens
            .map(u64::from)
            .unwrap_or_else(|| default_max_tokens(model_id)),
        "messages": converted.messages,
        "stream": stream,
    });
    if !converted.system.is_empty() {
        body["system"] = Value::Array(converted.system);
    }
    for (name, value) in [
        ("temperature", options.temperature),
        ("top_p", options.top_p),
    ] {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(ModelError::InvalidRequest(format!("{name} 必须是有限数值")));
            }
            body[name] = json!(value);
        }
    }
    if let Some(top_k) = options.top_k {
        body["top_k"] = json!(top_k);
    }
    if let Some(stop) = &options.stop_sequences {
        body["stop_sequences"] = json!(stop);
    }
    if let Some(effort) = options.reasoning {
        apply_reasoning(&mut body, effort);
    }
    if let Some(definitions) = &options.tools {
        body["tools"] = tools::definitions(definitions)?;
    }
    if let Some(choice) = &options.tool_choice {
        body["tool_choice"] = tools::choice(choice);
    }
    if let Some(format) = &options.response_format {
        if let Some(format) = output_format(format)? {
            merge_value(&mut body["output_config"], json!({ "format": format }));
        }
    }
    let betas = apply_provider_options(&mut body, options.provider_options.as_ref())?;
    Ok(BuiltRequest { body, betas })
}

fn default_max_tokens(model_id: &str) -> u64 {
    crate::ModelCatalog::builtin()
        .get("anthropic", model_id)
        .map(|profile| profile.limits.max_output_tokens)
        .unwrap_or(DEFAULT_MAX_TOKENS)
}

fn reject_unsupported(options: &CallOptions) -> Result<(), ModelError> {
    let unsupported = [
        (options.presence_penalty.is_some(), "presence_penalty"),
        (options.frequency_penalty.is_some(), "frequency_penalty"),
        (options.seed.is_some(), "seed"),
    ];
    if let Some((_, name)) = unsupported.into_iter().find(|(present, _)| *present) {
        return Err(ModelError::unsupported(
            name,
            Some("Messages API 不支持此通用参数".into()),
        ));
    }
    Ok(())
}

/// 通用推理档位映射为自适应思考加 `output_config.effort`。
///
/// 仅支持 `budget_tokens` 的旧模型请通过 `provider_options.anthropic.thinking`
/// 显式配置，它会覆盖这里的默认值。
fn apply_reasoning(body: &mut Value, effort: ReasoningEffort) {
    let level = match effort {
        ReasoningEffort::ProviderDefault => return,
        ReasoningEffort::None => {
            body["thinking"] = json!({ "type": "disabled" });
            return;
        }
        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        ReasoningEffort::Max => "max",
    };
    body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
    merge_value(&mut body["output_config"], json!({ "effort": level }));
}

fn output_format(format: &ResponseFormat) -> Result<Option<Value>, ModelError> {
    match format {
        ResponseFormat::Text => Ok(None),
        ResponseFormat::Json { schema: None, .. } => Err(ModelError::unsupported(
            "response_format.json",
            Some("Messages API 的 JSON 输出必须提供 schema".into()),
        )),
        ResponseFormat::Json {
            schema: Some(schema),
            ..
        } => Ok(Some(json!({ "type": "json_schema", "schema": schema }))),
    }
}

/// 合并 `provider_options.anthropic`，返回需要附加到 `anthropic-beta` 的标记。
fn apply_provider_options(
    body: &mut Value,
    options: Option<&ProviderOptions>,
) -> Result<Vec<String>, ModelError> {
    let Some(extra) = options.and_then(|p| p.inner.get("anthropic")) else {
        return Ok(Vec::new());
    };
    let fields = extra.as_object().ok_or_else(|| {
        ModelError::InvalidRequest("provider_options.anthropic 必须是对象".into())
    })?;
    let mut betas = Vec::new();
    for (key, value) in fields {
        match key.as_str() {
            "betas" => betas = beta_list(value)?,
            // 显式的思考配置整体替换通用推理档位生成的默认值。
            "thinking" => body["thinking"] = value.clone(),
            "output_config" | "metadata" | "context_management" => {
                merge_value(&mut body[key], value.clone())
            }
            "service_tier" | "mcp_servers" | "container" => body[key] = value.clone(),
            _ => {
                return Err(ModelError::unsupported(
                    key,
                    Some("Messages 扩展参数尚未支持".into()),
                ))
            }
        }
    }
    Ok(betas)
}

fn beta_list(value: &Value) -> Result<Vec<String>, ModelError> {
    value
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|items| items.iter().all(|item| !item.trim().is_empty()))
        .ok_or_else(|| {
            ModelError::InvalidRequest(
                "provider_options.anthropic.betas 必须是非空字符串数组".into(),
            )
        })
}

fn merge_value(target: &mut Value, incoming: Value) {
    match (target, incoming) {
        (Value::Object(existing), Value::Object(fields)) => existing.extend(fields),
        (target, value) => *target = value,
    }
}

/// 读取内容块级别的 `provider_options.anthropic.cache_control`。
pub(super) fn cache_control(options: Option<&ProviderOptions>) -> Option<Value> {
    options
        .and_then(|p| p.inner.get("anthropic"))
        .and_then(|p| p.get("cache_control"))
        .filter(|v| !v.is_null())
        .cloned()
}

/// 把 `cache_control` 附加到内容块上。
pub(super) fn with_cache_control(mut block: Value, cache: Option<Value>) -> Value {
    if let (Some(object), Some(cache)) = (block.as_object_mut(), cache) {
        object.insert("cache_control".into(), cache);
    }
    block
}

/// 读取某个厂商键下的指定字段。
pub(super) fn anthropic_field<'a>(
    options: Option<&'a ProviderOptions>,
    field: &str,
) -> Option<&'a Value> {
    options
        .and_then(|p| p.inner.get("anthropic"))
        .and_then(Value::as_object)
        .and_then(|fields| fields.get(field))
        .filter(|v| !v.is_null())
}
