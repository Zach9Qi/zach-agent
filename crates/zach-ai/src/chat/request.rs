//! Chat Completions 请求和消息映射。

mod media;
mod tools;

use serde_json::{json, Value};
use zach_ai_core::{
    AssistantPart, CallOptions, Message, ModelError, ReasoningEffort, ResponseFormat, ToolPart,
    UserPart,
};

pub(super) fn build_request(
    model_id: &str,
    options: &CallOptions,
    stream: bool,
) -> Result<Value, ModelError> {
    if model_id.trim().is_empty() {
        return Err(ModelError::InvalidRequest("模型 ID 不能为空".into()));
    }
    if options.top_k.is_some() {
        return Err(ModelError::unsupported(
            "top_k",
            Some("Chat Completions 不支持此参数".into()),
        ));
    }
    let mut body = json!({
        "model": model_id,
        "messages": messages(&options.prompt.messages)?,
        "stream": stream,
    });
    if let Some(value) = options.max_output_tokens {
        body["max_completion_tokens"] = json!(value);
    }
    for (name, value) in [
        ("temperature", options.temperature),
        ("top_p", options.top_p),
        ("presence_penalty", options.presence_penalty),
        ("frequency_penalty", options.frequency_penalty),
    ] {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(ModelError::InvalidRequest(format!("{name} 必须是有限数值")));
            }
            body[name] = json!(value);
        }
    }
    if let Some(stop) = &options.stop_sequences {
        body["stop"] = json!(stop);
    }
    if let Some(seed) = options.seed {
        body["seed"] = json!(seed);
    }
    if let Some(reasoning) = options.reasoning {
        if reasoning == ReasoningEffort::Max {
            return Err(ModelError::unsupported(
                "reasoning_effort.max",
                Some("Chat Completions 的最高推理档位是 xhigh，请改用 Xhigh".into()),
            ));
        }
        if reasoning != ReasoningEffort::ProviderDefault {
            body["reasoning_effort"] = json!(reasoning_name(reasoning));
        }
    }
    if let Some(tools_value) = &options.tools {
        body["tools"] = tools::definitions(tools_value)?;
    }
    if let Some(choice) = &options.tool_choice {
        body["tool_choice"] = tools::choice(choice);
    }
    if let Some(format) = &options.response_format {
        body["response_format"] = response_format(format);
    }
    if stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    if let Some(extra) = options
        .provider_options
        .as_ref()
        .and_then(|p| p.inner.get("openai"))
    {
        let fields = extra.as_object().ok_or_else(|| {
            ModelError::InvalidRequest("provider_options.openai 必须是对象".into())
        })?;
        for (key, value) in fields {
            if matches!(key.as_str(), "messages" | "model" | "stream") {
                return Err(ModelError::unsupported(
                    key,
                    Some("不能覆盖适配器管理的请求字段".into()),
                ));
            }
            body[key] = value.clone();
        }
    }
    Ok(body)
}

fn reasoning_name(value: ReasoningEffort) -> &'static str {
    match value {
        ReasoningEffort::None => "none",
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        // Max 与 ProviderDefault 已在 build_request 中提前过滤
        ReasoningEffort::Max | ReasoningEffort::ProviderDefault => {
            unreachable!("build_request 已过滤该档位")
        }
    }
}

fn messages(messages: &[Message]) -> Result<Vec<Value>, ModelError> {
    messages.iter().try_fold(Vec::new(), |mut all, item| {
        all.extend(message(item)?);
        Ok(all)
    })
}

fn message(message: &Message) -> Result<Vec<Value>, ModelError> {
    match message {
        Message::System { content, .. } => Ok(vec![json!({"role":"system", "content": content})]),
        Message::User { content, .. } => Ok(vec![
            json!({"role":"user", "content": user_content(content)?}),
        ]),
        Message::Assistant { content, .. } => assistant_message(content),
        Message::Tool { content, .. } => tool_message(content),
    }
}

fn user_content(parts: &[UserPart]) -> Result<Value, ModelError> {
    if parts
        .iter()
        .all(|part| matches!(part, UserPart::Text { .. }))
    {
        return Ok(Value::String(
            parts
                .iter()
                .filter_map(|part| match part {
                    UserPart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
        ));
    }
    parts
        .iter()
        .map(|part| match part {
            UserPart::Text { text, .. } => Ok(json!({"type":"text", "text":text})),
            UserPart::File {
                media_type,
                data,
                filename,
                ..
            } => media::input(media_type, data, filename.as_deref()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn assistant_message(parts: &[AssistantPart]) -> Result<Vec<Value>, ModelError> {
    let mut content = String::new();
    let mut refusal = None;
    let mut calls = Vec::new();
    for part in parts {
        match part {
            AssistantPart::Text {
                text,
                provider_options,
            } => {
                if provider_options
                    .as_ref()
                    .and_then(|p| p.inner.get("openai"))
                    .and_then(|v| v.get("refusal"))
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    refusal = Some(text.clone());
                } else {
                    content.push_str(text);
                }
            }
            AssistantPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                provider_executed,
                ..
            } => {
                if *provider_executed {
                    return Err(ModelError::unsupported("provider_executed_tool_call", None));
                }
                calls.push(json!({"id":tool_call_id, "type":"function", "function":{"name":tool_name, "arguments":serde_json::to_string(input)?}}));
            }
            // Chat Completions 的输入消息不接受思考链字段（DeepSeek 等端点
            // 收到 reasoning_content 会直接 400），历史推理不回传。
            AssistantPart::Reasoning { .. } => {}
            AssistantPart::ToolResult { .. }
            | AssistantPart::ReasoningFile { .. }
            | AssistantPart::File { .. }
            | AssistantPart::Custom { .. } => {
                return Err(ModelError::unsupported(
                    "assistant_part",
                    Some("Chat Completions 历史支持文本与函数调用".into()),
                ))
            }
        }
    }
    let mut result = json!({"role":"assistant", "content": if content.is_empty() && !calls.is_empty() { Value::Null } else { json!(content) }});
    if !calls.is_empty() {
        result["tool_calls"] = Value::Array(calls);
    }
    if let Some(refusal) = refusal {
        result["refusal"] = json!(refusal);
    }
    Ok(vec![result])
}

fn tool_message(parts: &[ToolPart]) -> Result<Vec<Value>, ModelError> {
    let mut results = Vec::new();
    for part in parts {
        match part {
            ToolPart::ToolResult {
                tool_call_id,
                output,
                ..
            } => {
                results.push(json!({"role":"tool", "tool_call_id":tool_call_id, "content":media::tool_output(output)?}));
            }
            ToolPart::ToolApprovalResponse { .. } => {
                return Err(ModelError::unsupported("tool_approval_response", None))
            }
        }
    }
    if results.is_empty() {
        Err(ModelError::InvalidRequest("工具消息不能为空".into()))
    } else {
        Ok(results)
    }
}

fn response_format(format: &ResponseFormat) -> Value {
    match format {
        ResponseFormat::Text => json!({"type":"text"}),
        ResponseFormat::Json { schema: None, .. } => json!({"type":"json_object"}),
        ResponseFormat::Json {
            schema: Some(schema),
            name,
            description,
        } => {
            let mut schema_value = json!({"name":name.as_deref().unwrap_or("response"), "schema":schema, "strict":true});
            if let Some(description) = description {
                schema_value["description"] = json!(description);
            }
            json!({"type":"json_schema", "json_schema":schema_value})
        }
    }
}
