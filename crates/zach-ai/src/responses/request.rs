//! Responses API 请求参数映射，消息回放与附件转换分别由子模块负责。

mod input;
mod media;
mod tools;

#[cfg(test)]
mod tests;

use serde_json::{json, Value};
use zach_ai_core::{
    CallOptions, ModelError, ModelProfile, ModelWarning, ReasoningEffort, ResponseFormat,
};

use crate::validate::{validate, Validated};

/// 已构建的请求：正文与档案校验产生的警告。
#[derive(Debug)]
pub(super) struct BuiltRequest {
    pub(super) body: Value,
    pub(super) warnings: Vec<ModelWarning>,
}

pub(super) fn build_request(
    model_id: &str,
    profile: Option<&ModelProfile>,
    options: &CallOptions,
    stream: bool,
) -> Result<BuiltRequest, ModelError> {
    if model_id.trim().is_empty() {
        return Err(ModelError::InvalidRequest("模型 ID 不能为空".into()));
    }
    reject_unsupported(options)?;
    let Validated {
        reasoning,
        temperature,
        warnings,
    } = validate(profile, options, &[])?;
    // Agent 自行管理历史，默认采用无状态模式并请求可回放的加密推理。
    let mut body = json!({
        "model": model_id,
        "input": input::messages(&options.prompt.messages)?,
        "stream": stream,
        "store": false,
        "include": ["reasoning.encrypted_content"]
    });
    if let Some(value) = options.max_output_tokens {
        body["max_output_tokens"] = json!(value);
    }
    for (name, value) in [("temperature", temperature), ("top_p", options.top_p)] {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(ModelError::InvalidRequest(format!("{name} 必须是有限数值")));
            }
            body[name] = json!(value);
        }
    }
    if let Some(effort) = reasoning {
        if effort == ReasoningEffort::Max {
            return Err(ModelError::unsupported(
                "reasoning_effort.max",
                Some("Responses API 的最高推理档位是 xhigh，请改用 Xhigh".into()),
            ));
        }
        if effort != ReasoningEffort::ProviderDefault {
            body["reasoning"] = json!({ "effort": effort });
        }
    }
    if let Some(definitions) = &options.tools {
        body["tools"] = tools::definitions(definitions)?;
    }
    if let Some(choice) = &options.tool_choice {
        body["tool_choice"] = tools::choice(choice);
    }
    if let Some(format) = &options.response_format {
        body["text"] = json!({ "format": response_format(format) });
    }
    if let Some(extra) = options
        .provider_options
        .as_ref()
        .and_then(|p| p.inner.get("openai"))
    {
        let fields = extra.as_object().ok_or_else(|| {
            ModelError::InvalidRequest("provider_options.openai 必须是对象".into())
        })?;
        // 仅接受不改变本适配器运行方式的扩展。状态续接和后台任务需独立契约。
        for (key, value) in fields {
            if !matches!(
                key.as_str(),
                "reasoning"
                    | "text"
                    | "parallel_tool_calls"
                    | "metadata"
                    | "service_tier"
                    | "truncation"
                    | "store"
                    | "include"
                    | "user"
                    | "safety_identifier"
                    | "prompt_cache_key"
                    | "prompt_cache_retention"
            ) {
                return Err(ModelError::unsupported(
                    key,
                    Some("Responses 扩展参数尚未支持".into()),
                ));
            }
            merge_value(&mut body[key], value.clone());
        }
    }
    if let Some(include) = body.get_mut("include") {
        let values = include
            .as_array_mut()
            .ok_or_else(|| ModelError::InvalidRequest("include 必须是字符串数组".into()))?;
        if values.iter().any(|v| !v.is_string()) {
            return Err(ModelError::InvalidRequest(
                "include 必须是字符串数组".into(),
            ));
        }
        let encrypted = json!("reasoning.encrypted_content");
        if !values.contains(&encrypted) {
            values.push(encrypted);
        }
    }
    Ok(BuiltRequest { body, warnings })
}

fn merge_value(target: &mut Value, incoming: Value) {
    match (target, incoming) {
        (Value::Object(existing), Value::Object(fields)) => existing.extend(fields),
        (target, value) => *target = value,
    }
}

fn reject_unsupported(options: &CallOptions) -> Result<(), ModelError> {
    let unsupported = [
        (options.top_k.is_some(), "top_k"),
        (options.presence_penalty.is_some(), "presence_penalty"),
        (options.frequency_penalty.is_some(), "frequency_penalty"),
        (options.stop_sequences.is_some(), "stop_sequences"),
        (options.seed.is_some(), "seed"),
    ];
    if let Some((_, name)) = unsupported.into_iter().find(|(present, _)| *present) {
        return Err(ModelError::unsupported(
            name,
            Some("Responses API 不支持此通用参数".into()),
        ));
    }
    Ok(())
}

fn response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({ "type": "text" }),
        ResponseFormat::Json { schema: None, .. } => json!({ "type": "json_object" }),
        ResponseFormat::Json {
            schema: Some(schema),
            name,
            description,
        } => {
            let mut format = json!({
                "type": "json_schema",
                "name": name.as_deref().unwrap_or("response"),
                "schema": schema,
                "strict": true,
            });
            if let Some(description) = description {
                format["description"] = json!(description);
            }
            format
        }
    }
}
