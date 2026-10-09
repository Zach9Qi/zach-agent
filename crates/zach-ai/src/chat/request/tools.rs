//! Chat Completions 函数声明与工具选择。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ToolChoice, ToolDefinition};

pub(super) fn definitions(definitions: &[ToolDefinition]) -> Result<Value, ModelError> {
    definitions.iter().map(|definition| {
        let ToolDefinition::Function(tool) = definition else { return Err(ModelError::unsupported("provider_tool", Some("当前适配器只支持函数工具".into()))); };
        if tool.defer_loading == Some(true) || tool.input_examples.is_some() { return Err(ModelError::unsupported("defer_loading/input_examples", None)); }
        let mut function = json!({"name":tool.name, "parameters":tool.input_schema, "strict":tool.strict.unwrap_or(false)});
        if let Some(description) = &tool.description { function["description"] = json!(description); }
        Ok(json!({"type":"function", "function":function}))
    }).collect::<Result<Vec<_>, _>>().map(Value::Array)
}

pub(super) fn choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { tool_name } => json!({"type":"function", "function":{"name":tool_name}}),
    }
}
