//! 厂商原生工具（ProviderTool）序列化与 ToolDefinition 包装测试

use serde_json::json;
use zach_ai_core::{ProviderTool, ToolDefinition};

#[test]
fn test_provider_tool_serde_and_definition() {
    let tool = ProviderTool::new(
        "openai.web_search",
        "web_search",
        json!({ "search_context_size": "medium" }),
    );

    let json_val = serde_json::to_value(&tool).expect("序列化 ProviderTool 失败");
    assert_eq!(json_val["id"], "openai.web_search");
    assert_eq!(json_val["name"], "web_search");
    assert_eq!(json_val["args"]["search_context_size"], "medium");

    let deserialized: ProviderTool =
        serde_json::from_value(json_val).expect("反序列化 ProviderTool 失败");
    assert_eq!(tool, deserialized);

    let tool_def: ToolDefinition = tool.clone().into();
    assert!(tool_def.is_provider());
    assert!(!tool_def.is_function());
    assert_eq!(tool_def.name(), "web_search");
    assert_eq!(tool_def.description(), None);
    assert_eq!(
        tool_def.as_provider().map(|p| p.id.as_str()),
        Some("openai.web_search")
    );

    // ToolDefinition 带 type tag
    let def_json = serde_json::to_value(&tool_def).expect("序列化 ToolDefinition 失败");
    assert_eq!(def_json["type"], "provider");
    assert_eq!(def_json["id"], "openai.web_search");
}
