//! Messages 工具声明与工具选择的映射：函数工具字段透传、服务端工具与非法声明。

use super::*;
use zach_ai_core::{FunctionTool, ProviderTool, ToolChoice, ToolDefinition};

#[test]
fn tools_and_tool_choice_use_messages_shapes() {
    let function = FunctionTool::new("add", json!({"type": "object"}))
        .with_description("加法")
        .with_strict(true)
        .with_defer_loading(true)
        .with_input_examples(vec![json!({"a": 1})]);
    let search = ProviderTool::new(
        "anthropic.web_search",
        "web_search",
        json!({"type": "web_search_20250305", "max_uses": 3}),
    );
    let options = CallOptions::new(vec![Message::user("x")])
        .with_tools(vec![function.into(), search.into()])
        .with_tool_choice(ToolChoice::Required);
    let body = build(&options);
    assert_eq!(
        body["tools"][0],
        json!({"name": "add", "description": "加法", "input_schema": {"type": "object"},
            "strict": true, "defer_loading": true, "input_examples": [{"a": 1}]})
    );
    assert_eq!(
        body["tools"][1],
        json!({"type": "web_search_20250305", "max_uses": 3, "name": "web_search"})
    );
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    for (choice, expected) in [
        (ToolChoice::Auto, json!({"type": "auto"})),
        (ToolChoice::None, json!({"type": "none"})),
        (
            ToolChoice::specific("add"),
            json!({"type": "tool", "name": "add"}),
        ),
    ] {
        assert_eq!(tools::choice(&choice), expected);
    }
    let foreign: ToolDefinition =
        ProviderTool::new("openai.web_search", "web_search", json!({})).into();
    assert!(tools::definitions(&[foreign]).is_err());
    let untyped: ToolDefinition = ProviderTool::new("anthropic.bash", "bash", json!({})).into();
    assert!(tools::definitions(&[untyped]).is_err());
}
