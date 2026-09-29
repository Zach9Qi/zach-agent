//! 工具入参：把模型流式拼接出的原始字符串解析为结构化 JSON Object
//!
//! 同一份原始入参有三类消费者，对"解析失败"的期望各不相同：
//!
//! - 回放到下一轮 Prompt：必须是 Object，否则厂商会拒绝整个请求 → [`tool_input_for_replay`]
//! - 执行前校验：必须报错，并把原因回传给模型 → [`parse_tool_input`]
//! - 事件展示 / 审批弹窗：保留原文最便于人眼诊断 → [`tool_input_for_display`]
//!
//! [`parse_tool_input`] 是唯一的真相源，另外两个只是在失败分支上做不同的兜底。

use serde_json::{Map, Value};

/// 工具入参不是合法的 JSON 对象
///
/// 内部字符串仅承载诊断细节（serde 报错、实际的 JSON 类型等），不承担分类职责：
/// 无论是截断、畸形还是类型不对，对调用方而言都是同一个事实，处理方式也一致。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("工具入参必须是合法的 JSON 对象: {0}")]
pub struct ToolInputError(String);

/// 严格解析原始入参
///
/// - 空白串视为 `{}`（无参工具在流式场景下的常态）；
/// - 其余必须是合法的 JSON Object；
/// - 非法 JSON、以及数组 / 数字 / 字符串 / null 等非对象值一律报错。
pub fn parse_tool_input(raw: &str) -> Result<Value, ToolInputError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(empty_object());
    }
    let value: Value = serde_json::from_str(trimmed).map_err(|e| ToolInputError(e.to_string()))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(ToolInputError(format!("实际为 {}", json_type_name(&value))))
    }
}

/// 回放到下一轮 Prompt 时使用：解析失败一律归一为 `{}`
///
/// 这是 `AssistantPart::ToolCall.input` "永远是 JSON Object" 契约的唯一守门点。
/// 失败原因不会在这里丢失——与该调用配对的 `ToolResult` 会把错误告诉模型。
pub fn tool_input_for_replay(raw: &str) -> Value {
    parse_tool_input(raw).unwrap_or_else(|_| empty_object())
}

/// 展示给宿主 / UI 时使用：解析失败保留原文
pub fn tool_input_for_display(raw: &str) -> Value {
    parse_tool_input(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "布尔值",
        Value::Number(_) => "数字",
        Value::String(_) => "字符串",
        Value::Array(_) => "数组",
        Value::Object(_) => "对象",
    }
}
