//! Responses 函数声明与工具选择映射。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ToolChoice, ToolDefinition};

pub(super) fn definitions(definitions: &[ToolDefinition]) -> Result<Value, ModelError> {
    definitions
        .iter()
        .map(|definition| {
            let ToolDefinition::Function(tool) = definition else {
                return Err(ModelError::unsupported(
                    "provider_tool",
                    Some("当前 Responses 适配器只支持本地函数工具".into()),
                ));
            };
            if tool.defer_loading == Some(true) || tool.input_examples.is_some() {
                return Err(ModelError::unsupported(
                    "defer_loading/input_examples",
                    None,
                ));
            }
            let mut value = json!({
                "type": "function", "name": tool.name, "parameters": tool.input_schema,
                // 保留通用 schema 的可选字段语义；严格模式由调用者显式选择。
                "strict": tool.strict.unwrap_or(false),
            });
            if let Some(description) = &tool.description {
                value["description"] = json!(description);
            }
            Ok(value)
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

pub(super) fn choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { tool_name } => json!({"type": "function", "name": tool_name}),
    }
}
