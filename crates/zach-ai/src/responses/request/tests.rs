//! 请求转换覆盖消息顺序、推理回放、附件与协议不支持项。

use super::*;
use zach_ai_core::{
    AssistantPart, FileData, FunctionTool, Message, Prompt, ProviderOptions, ProviderTool,
    ToolChoice, ToolPart, UserPart,
};

#[test]
fn history_is_flat_and_keeps_reasoning_call_and_result_order() {
    let reasoning = json!({"type": "reasoning", "id": "rs_1",
        "summary": [{"type": "summary_text", "text": "摘要"}],
        "encrypted_content": "opaque"});
    let mut metadata = ProviderOptions::new();
    metadata.insert("openai", json!({"responses_item": reasoning}));
    let options = CallOptions::new(vec![
        Message::user("计算"),
        Message::assistant_tool_calls(vec![
            AssistantPart::text("开始"),
            AssistantPart::Reasoning {
                text: "摘要".into(),
                provider_options: Some(metadata),
            },
            AssistantPart::tool_call("call_1", "add", json!({"a": 1})),
            AssistantPart::text("调用中"),
        ]),
        Message::tool(vec![ToolPart::result_json(
            "call_1",
            "add",
            json!({"sum": 1}),
        )]),
    ]);
    let body = build_request("model", &options, true).unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 6);
    assert!(input.iter().all(Value::is_object));
    assert_eq!(input[1]["content"], "开始");
    assert_eq!(input[2], reasoning);
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(input[3]["arguments"], r#"{"a":1}"#);
    assert_eq!(input[4]["content"], "调用中");
    assert_eq!(input[5]["type"], "function_call_output");
    assert_eq!(input[5]["output"], r#"{"sum":1}"#);
    assert_eq!(body["store"], false);
}

#[test]
fn unmarked_reasoning_is_never_sent_as_assistant_text() {
    let options = CallOptions::new(vec![Message::assistant_tool_calls(vec![
        AssistantPart::reasoning("其他协议摘要"),
        AssistantPart::text("答案"),
    ])]);
    let body = build_request("model", &options, false).unwrap();
    assert_eq!(body["input"].as_array().unwrap().len(), 1);
    assert_eq!(body["input"][0]["content"], "答案");
}

#[test]
fn files_use_distinct_url_data_and_reference_fields() {
    let parts = vec![
        UserPart::file("image/png", FileData::from_bytes(vec![1, 2, 3])),
        UserPart::file(
            "image/png",
            FileData::from_reference("openai", "file_image"),
        ),
        UserPart::File {
            media_type: "application/pdf".into(),
            data: FileData::from_bytes(vec![1, 2, 3]),
            filename: Some("invoice.pdf".into()),
            provider_options: None,
        },
        UserPart::file(
            "application/pdf",
            FileData::from_url("https://example.com/a.pdf"),
        ),
    ];
    let options = CallOptions::new(vec![Message::User {
        content: parts,
        provider_options: None,
    }]);
    let body = build_request("model", &options, false).unwrap();
    let content = &body["input"][0]["content"];
    assert_eq!(content[0]["image_url"], "data:image/png;base64,AQID");
    assert_eq!(content[1]["file_id"], "file_image");
    assert!(content[1].get("image_url").is_none());
    assert_eq!(content[2]["file_data"], "data:application/pdf;base64,AQID");
    assert_eq!(content[2]["filename"], "invoice.pdf");
    assert_eq!(content[3]["file_url"], "https://example.com/a.pdf");
}

#[test]
fn foreign_file_reference_fails_instead_of_using_an_arbitrary_id() {
    let options = CallOptions::new(vec![Message::user_with_file(
        "读取",
        "application/pdf",
        FileData::from_reference("anthropic", "other-file"),
    )]);
    assert!(matches!(
        build_request("model", &options, false),
        Err(ModelError::InvalidRequest(_))
    ));
}

#[test]
fn tools_and_json_modes_use_responses_shapes() {
    let mut options = CallOptions::new(Prompt::new().with_system("系统").with_user("JSON"))
        .with_tools(vec![
            FunctionTool::new("add", json!({"type": "object"})).into()
        ])
        .with_tool_choice(ToolChoice::specific("add"));
    options.response_format = Some(ResponseFormat::Json {
        schema: None,
        name: None,
        description: None,
    });
    let body = build_request("model", &options, true).unwrap();
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["strict"], false);
    assert!(body["tools"][0].get("function").is_none());
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "name": "add"})
    );
    assert_eq!(body["text"]["format"]["type"], "json_object");
    options.response_format = Some(ResponseFormat::json_schema(json!({"type": "object"})));
    let body = build_request("model", &options, false).unwrap();
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert!(body["text"]["format"].get("description").is_none());
}

#[test]
fn provider_options_merge_reasoning_and_keep_replay_enabled() {
    let mut options = CallOptions::default().with_reasoning(ReasoningEffort::High);
    let mut provider = ProviderOptions::new();
    provider.insert(
        "openai",
        json!({"reasoning": {"summary": "auto"}, "include": []}),
    );
    options.provider_options = Some(provider);
    let body = build_request("model", &options, false).unwrap();
    assert_eq!(
        body["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
}

#[test]
fn unsupported_parameters_and_hosted_tools_are_rejected_before_sending() {
    let mut options = CallOptions {
        seed: Some(1),
        ..Default::default()
    };
    assert!(matches!(
        build_request("model", &options, false),
        Err(ModelError::UnsupportedFeature { .. })
    ));
    options.seed = None;
    options.tools = Some(vec![ProviderTool::new(
        "openai.web_search",
        "web_search",
        json!({}),
    )
    .into()]);
    assert!(matches!(
        build_request("model", &options, false),
        Err(ModelError::UnsupportedFeature { .. })
    ));
    options.tools = None;
    let mut provider = ProviderOptions::new();
    provider.insert("openai", json!({"background": true}));
    options.provider_options = Some(provider);
    assert!(build_request("model", &options, true).is_err());
    options
        .provider_options
        .as_mut()
        .unwrap()
        .insert("openai", json!("bad"));
    assert!(build_request("model", &options, true).is_err());
}
