//! Chat Completions 请求参数映射：工具结果、推理档位、输出上限、文本部件、输出格式与扩展参数。

use super::*;
use crate::test_support::{profile, reasoning_profile};
use zach_ai_core::Message;

/// tool 消息的 content 只接受文本部件，文件附件必须在发送前被拒绝而非生成非法部件。
#[test]
fn tool_result_file_blocks_are_rejected_instead_of_emitting_invalid_parts() {
    use zach_ai_core::{FileData, ToolPart, ToolResultContentBlock, ToolResultOutput};
    let result = |blocks| {
        Message::tool(vec![ToolPart::ToolResult {
            tool_call_id: "call_1".into(),
            tool_name: "render".into(),
            output: ToolResultOutput::Content { value: blocks },
            provider_options: None,
        }])
    };
    let image = CallOptions::new(vec![result(vec![ToolResultContentBlock::File {
        media_type: "image/png".into(),
        data: FileData::from_bytes(vec![1, 2, 3]),
        filename: None,
        provider_options: None,
    }])]);
    assert!(matches!(
        build_request("gpt-test", "openai", None, &image, false),
        Err(zach_ai_core::ModelError::UnsupportedFeature { .. })
    ));
    let text = CallOptions::new(vec![result(vec![ToolResultContentBlock::File {
        media_type: "text/plain".into(),
        data: FileData::Text {
            text: "日志内容".into(),
        },
        filename: None,
        provider_options: None,
    }])]);
    let body = build_request("gpt-test", "openai", None, &text, false)
        .unwrap()
        .body;
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type":"text", "text":"日志内容"}])
    );
}

/// `reasoning_effort: "none"` 只是 OpenAI 官方端点的约定，第三方端点关闭思考的字段各不相同，
/// 不能假装发出去就关掉了：改为不发送并给出警告，提示改用扩展参数。
#[test]
fn disabling_reasoning_on_a_third_party_provider_warns_instead_of_sending_none() {
    use zach_ai_core::{ModelWarning, ReasoningEffort};
    let options = CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::None);
    let built = build_request("qwen3", "alibaba", None, &options, false).unwrap();
    assert!(built.body.get("reasoning_effort").is_none());
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Compatibility { feature, details: Some(details) }
            if feature == "reasoning_effort.none" && details.contains("provider_options.alibaba")
    ));
    let built = build_request("gpt-5.1", "openai", None, &options, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "none");
    assert!(built.warnings.is_empty());
    // 其他档位仍按 reasoning_effort 发送，由服务端裁决。
    let high = CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::High);
    let built = build_request("qwen3", "alibaba", None, &high, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "high");
    assert!(built.warnings.is_empty());
}

/// OpenAI 官方端点只认 `max_completion_tokens`，第三方兼容端点普遍只认 `max_tokens`。
#[test]
fn output_limit_field_follows_the_declared_provider() {
    let options = CallOptions::new(vec![Message::user("x")]).with_max_output_tokens(64);
    let openai = build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(openai["max_completion_tokens"], 64);
    assert!(openai.get("max_tokens").is_none());
    let deepseek = build_request("deepseek-chat", "deepseek", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(deepseek["max_tokens"], 64);
    assert!(deepseek.get("max_completion_tokens").is_none());
}

/// 多段文本保留部件边界（与 Responses 适配器一致），单段仍用字符串兼容老端点。
#[test]
fn multiple_user_text_parts_keep_their_boundaries() {
    use zach_ai_core::UserPart;
    let options = CallOptions::new(vec![Message::User {
        content: vec![UserPart::text("第一段"), UserPart::text("第二段")],
        provider_options: None,
    }]);
    let body = build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(
        body["messages"][0]["content"],
        json!([{"type": "text", "text": "第一段"}, {"type": "text", "text": "第二段"}])
    );
}

/// JSON Schema 输出的严格模式由调用方显式选择：普通 Schema 在严格模式下会被服务端拒绝。
#[test]
fn json_schema_output_is_strict_only_when_requested() {
    use zach_ai_core::ResponseFormat;
    let schema = json!({"type": "object", "properties": {"v": {"type": "integer"}}});
    let mut options = CallOptions::new(vec![Message::user("x")]);
    options.response_format = Some(ResponseFormat::json_schema(schema.clone()));
    let body = build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
    assert!(body["response_format"]["json_schema"]
        .get("strict")
        .is_none());
    options.response_format = Some(ResponseFormat::json_schema(schema).with_strict(true));
    let body = build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    options.response_format = Some(ResponseFormat::Json {
        schema: None,
        name: None,
        description: None,
        strict: None,
    });
    let body = build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["response_format"], json!({"type": "json_object"}));
}

/// 兼容端点的私有字段透传，但已由通用参数写入的字段不能被扩展参数悄悄覆盖。
#[test]
fn provider_options_pass_through_but_cannot_override_adapter_fields() {
    let mut options = CallOptions::new(vec![Message::user("你好")]).with_temperature(0.5);
    let mut provider = zach_ai_core::ProviderOptions::new();
    provider.insert(
        "openai",
        json!({"enable_thinking": true, "chat_template_kwargs": {"x": 1}, "max_tokens": 64}),
    );
    options.provider_options = Some(provider);
    let body = build_request("qwen", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["enable_thinking"], true);
    assert_eq!(body["chat_template_kwargs"], json!({"x": 1}));
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["temperature"], 0.5);
    for (key, value) in [
        ("temperature", json!(1.0)),
        ("messages", json!([])),
        ("model", json!("other")),
        ("stream", json!(true)),
    ] {
        let mut provider = zach_ai_core::ProviderOptions::new();
        provider.insert("openai", json!({ key: value }));
        options.provider_options = Some(provider);
        assert!(
            matches!(
                build_request("qwen", "openai", None, &options, false),
                Err(zach_ai_core::ModelError::UnsupportedFeature { feature, .. }) if feature == key
            ),
            "{key} 不应被覆盖"
        );
    }
}

/// `max` 档位由档案裁决而非一刀切拒绝：声明支持的模型照常发送，仅到 xhigh 的模型降级并
/// 给出警告；档案未知时原样发送由服务端裁决。不支持且无别名时的报错由 `validate` 单元测试覆盖。
#[test]
fn max_effort_follows_the_profile_instead_of_a_blanket_rejection() {
    use zach_ai_core::ReasoningEffort::{High, Max, Xhigh};
    let mut options = CallOptions::new(vec![Message::user("你好")]);
    options.reasoning = Some(Max);
    let newest = profile(
        "openai",
        "supports-max",
        reasoning_profile(&[High, Xhigh, Max], true),
        true,
    );
    let built = build_request(&newest.id, "openai", Some(&newest), &options, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "max");
    assert!(built.warnings.is_empty());
    let xhigh_only = profile(
        "openai",
        "xhigh-only",
        reasoning_profile(&[High, Xhigh], true),
        true,
    );
    let built =
        build_request(&xhigh_only.id, "openai", Some(&xhigh_only), &options, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "xhigh");
    assert!(matches!(
        &built.warnings[0],
        zach_ai_core::ModelWarning::Compatibility { feature, .. }
            if feature == "reasoning_effort.max"
    ));
    let unknown = build_request("proxy-model", "openai", None, &options, false).unwrap();
    assert_eq!(unknown.body["reasoning_effort"], "max");
}

/// 扩展参数优先读声明厂商的键，未提供时回退协议方 `openai` 的键。
#[test]
fn provider_options_read_the_declared_provider_key_and_fall_back_to_openai() {
    use zach_ai_core::ProviderOptions;
    let mut options = CallOptions::new(vec![Message::user("x")]);
    let mut provider = ProviderOptions::new();
    provider.insert("deepseek", json!({"thinking": {"type": "disabled"}}));
    provider.insert("openai", json!({"ignored": true}));
    options.provider_options = Some(provider);
    let built = build_request("deepseek-chat", "deepseek", None, &options, false).unwrap();
    assert_eq!(built.body["thinking"], json!({"type": "disabled"}));
    assert!(built.body.get("ignored").is_none());
    let mut provider = ProviderOptions::new();
    provider.insert("openai", json!({"enable_thinking": false}));
    options.provider_options = Some(provider);
    let built = build_request("deepseek-chat", "deepseek", None, &options, false).unwrap();
    assert_eq!(built.body["enable_thinking"], false);
}
