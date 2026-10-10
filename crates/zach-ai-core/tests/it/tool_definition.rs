//! `ToolDefinition` 的两种来源：函数工具与厂商原生工具的序列化往返、可选字段与包装访问器

use serde_json::json;
use zach_ai_core::{FunctionTool, ProviderTool, ToolDefinition};

#[test]
fn function_tool_serializes_optional_fields_only_when_set_and_wraps_into_definition() {
    let tool = FunctionTool::new(
        "query_database",
        json!({
            "type": "object",
            "properties": {
                "sql": { "type": "string" }
            },
            "required": ["sql"]
        }),
    )
    .with_description("执行 SQL 数据库查询")
    .with_defer_loading(true)
    .with_strict(true);

    // 验证序列化结果中包含 defer_loading: true
    let json_val = serde_json::to_value(&tool).expect("序列化 FunctionTool 失败");
    assert_eq!(json_val["name"], "query_database");
    assert_eq!(json_val["description"], "执行 SQL 数据库查询");
    assert_eq!(json_val["defer_loading"], true);
    assert_eq!(json_val["strict"], true);
    assert!(json_val["input_schema"].is_object());

    // 验证反序列化
    let deserialized: FunctionTool =
        serde_json::from_value(json_val).expect("反序列化 FunctionTool 失败");
    assert_eq!(tool, deserialized);
    assert_eq!(deserialized.defer_loading, Some(true));

    // 验证未设置 defer_loading 时不序列化该字段
    let normal_tool = FunctionTool::new("echo", json!({ "type": "object" }));
    let normal_json = serde_json::to_value(&normal_tool).expect("序列化普通 FunctionTool 失败");
    assert!(normal_json.get("defer_loading").is_none());

    // 验证 ToolDefinition 包装
    let tool_def: ToolDefinition = tool.clone().into();
    assert!(tool_def.is_function());
    assert_eq!(tool_def.name(), "query_database");
    assert_eq!(tool_def.description(), Some("执行 SQL 数据库查询"));
    assert_eq!(
        tool_def.as_function().and_then(|f| f.defer_loading),
        Some(true)
    );
}

#[test]
fn provider_tool_round_trips_and_wraps_into_a_tagged_definition() {
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
