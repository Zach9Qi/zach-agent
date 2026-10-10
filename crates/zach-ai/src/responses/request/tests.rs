//! 请求转换覆盖消息顺序、推理回放、附件与协议不支持项。

use super::*;
use crate::test_support::{profile, reasoning_profile};
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
    let body = build_request("model", None, &options, true).unwrap().body;
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
    let body = build_request("model", None, &options, false).unwrap().body;
    assert_eq!(body["input"].as_array().unwrap().len(), 1);
    assert_eq!(body["input"][0]["content"], "答案");
}

/// 无状态回放加密推理时 reasoning 项必须跟随按 id 配对的 message 项，
/// 因此带 item_id 的助手文本要以原始 message 形状回放并合并相邻分段。
#[test]
fn text_with_item_id_replays_as_message_item_and_merges_adjacent_parts() {
    let text_part = |text: &str, item_id: &str, refusal: bool| {
        let mut metadata = ProviderOptions::new();
        metadata.insert(
            "openai",
            json!({"item_id": item_id, "refusal": refusal, "phase": Value::Null}),
        );
        AssistantPart::Text {
            text: text.into(),
            provider_options: Some(metadata),
        }
    };
    let options = CallOptions::new(vec![Message::assistant_tool_calls(vec![
        text_part("前半", "msg_1", false),
        text_part("无法回答", "msg_1", true),
        text_part("另一条", "msg_2", false),
        AssistantPart::text("手工文本"),
    ])]);
    let body = build_request("model", None, &options, false).unwrap().body;
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert_eq!(
        input[0],
        json!({"type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "前半", "annotations": []},
                        {"type": "refusal", "refusal": "无法回答"}]})
    );
    assert_eq!(input[1]["id"], "msg_2");
    assert_eq!(
        input[1]["content"],
        json!([{"type": "output_text", "text": "另一条", "annotations": []}])
    );
    assert_eq!(
        input[2],
        json!({"role": "assistant", "content": "手工文本"})
    );
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
    let body = build_request("model", None, &options, false).unwrap().body;
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
        build_request("model", None, &options, false),
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
        strict: None,
    });
    let body = build_request("model", None, &options, true).unwrap().body;
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["strict"], false);
    assert!(body["tools"][0].get("function").is_none());
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "name": "add"})
    );
    assert_eq!(body["text"]["format"]["type"], "json_object");
    let schema = json!({"type":"object", "properties":{"value":{"type":"integer"}},
        "required":["value"], "additionalProperties":false});
    options.response_format = Some(ResponseFormat::json_schema(schema.clone()));
    let body = build_request("model", None, &options, false).unwrap().body;
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert_eq!(body["text"]["format"]["schema"], schema);
    // 严格模式与函数工具一致，由调用方显式选择。
    assert!(body["text"]["format"].get("strict").is_none());
    assert!(body["text"]["format"].get("description").is_none());
    options.response_format = Some(ResponseFormat::json_schema(schema).with_strict(true));
    let body = build_request("model", None, &options, false).unwrap().body;
    assert_eq!(body["text"]["format"]["strict"], true);
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
    let body = build_request("model", None, &options, false).unwrap().body;
    assert_eq!(
        body["reasoning"],
        json!({"effort": "high", "summary": "auto"})
    );
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
}

/// 档案声明不接受 temperature 或不支持某档位时，在发送前丢弃或报错，而不是让服务端返回 400；
/// `max` 按别名降级为 xhigh。
#[test]
fn profile_rejects_or_drops_settings_before_sending() {
    use zach_ai_core::ModelWarning;
    use zach_ai_core::ReasoningEffort::{High, Max, Xhigh};
    let locked = profile(
        "openai",
        "no-temperature",
        reasoning_profile(&[High], false),
        false,
    );
    let mut options = CallOptions::new(vec![Message::user("x")])
        .with_temperature(0.5)
        .with_reasoning(Xhigh);
    assert!(matches!(
        build_request(&locked.id, Some(&locked), &options, false),
        Err(ModelError::UnsupportedFeature { feature, .. }) if feature == "reasoning_effort.xhigh"
    ));
    options.reasoning = Some(High);
    let built = build_request(&locked.id, Some(&locked), &options, false).unwrap();
    assert!(built.body.get("temperature").is_none());
    assert_eq!(built.body["reasoning"]["effort"], "high");
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Unsupported { feature, .. } if feature == "temperature"
    ));
    let plain = profile("openai", "no-reasoning", None, true);
    let built = build_request(&plain.id, Some(&plain), &options, false).unwrap();
    assert!(built.body.get("reasoning").is_none());
    assert_eq!(built.body["temperature"], 0.5);
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Unsupported { feature, .. } if feature == "reasoning"
    ));
    options.temperature = None;
    options.reasoning = Some(Max);
    let xhigh_only = profile(
        "openai",
        "xhigh-only",
        reasoning_profile(&[High, Xhigh], true),
        true,
    );
    let built = build_request(&xhigh_only.id, Some(&xhigh_only), &options, false).unwrap();
    assert_eq!(built.body["reasoning"]["effort"], "xhigh");
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Compatibility { feature, .. } if feature == "reasoning_effort.max"
    ));
}

#[test]
fn unsupported_parameters_and_hosted_tools_are_rejected_before_sending() {
    let mut options = CallOptions {
        seed: Some(1),
        ..Default::default()
    };
    assert!(matches!(
        build_request("model", None, &options, false),
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
        build_request("model", None, &options, false),
        Err(ModelError::UnsupportedFeature { .. })
    ));
    options.tools = None;
    let mut provider = ProviderOptions::new();
    provider.insert("openai", json!({"background": true}));
    options.provider_options = Some(provider);
    assert!(build_request("model", None, &options, true).is_err());
    options
        .provider_options
        .as_mut()
        .unwrap()
        .insert("openai", json!("bad"));
    assert!(build_request("model", None, &options, true).is_err());
}
