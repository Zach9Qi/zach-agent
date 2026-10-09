//! Messages 工具声明与工具选择映射。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ToolChoice, ToolDefinition};

use super::{cache_control, with_cache_control};

pub(super) fn definitions(definitions: &[ToolDefinition]) -> Result<Value, ModelError> {
    definitions
        .iter()
        .map(|definition| match definition {
            ToolDefinition::Function(tool) => {
                let mut value = json!({
                    "name": tool.name,
                    "input_schema": tool.input_schema,
                });
                if let Some(description) = &tool.description {
                    value["description"] = json!(description);
                }
                if let Some(strict) = tool.strict {
                    value["strict"] = json!(strict);
                }
                if let Some(defer) = tool.defer_loading {
                    value["defer_loading"] = json!(defer);
                }
                if let Some(examples) = &tool.input_examples {
                    value["input_examples"] = json!(examples);
                }
                Ok(with_cache_control(
                    value,
                    cache_control(tool.provider_options.as_ref()),
                ))
            }
            ToolDefinition::Provider(tool) => {
                // 服务端工具的参数即 Anthropic 工具定义本身（必须含版本化的 `type`）。
                let Some(kind) = tool.id.strip_prefix("anthropic.") else {
                    return Err(ModelError::unsupported(
                        "provider_tool",
                        Some(format!("{} 不是 Anthropic 服务端工具", tool.id)),
                    ));
                };
                let mut value = match &tool.args {
                    Value::Object(args) => Value::Object(args.clone()),
                    Value::Null => json!({}),
                    _ => {
                        return Err(ModelError::InvalidRequest(format!(
                            "服务端工具 {kind} 的参数必须是对象"
                        )))
                    }
                };
                if value.get("type").and_then(Value::as_str).is_none() {
                    return Err(ModelError::InvalidRequest(format!(
                        "服务端工具 {kind} 缺少版本化的 type 字段"
                    )));
                }
                value["name"] = json!(tool.name);
                Ok(value)
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

pub(super) fn choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::None => json!({"type": "none"}),
        ToolChoice::Required => json!({"type": "any"}),
        ToolChoice::Tool { tool_name } => json!({"type": "tool", "name": tool_name}),
    }
}
