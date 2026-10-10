//! Chat Completions 请求映射、非流式结果与模型身份测试；流式生命周期见 `stream/tests.rs`。

use super::*;
use serde_json::{json, Value};
use zach_ai_core::{CallOptions, LanguageModel, Message};

#[test]
fn request_uses_chat_endpoint_and_openai_message_shapes() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test")
        .with_base_url("https://example.test/v1/");
    let options = CallOptions::new(vec![Message::system("规则"), Message::user("你好")]);
    let (request, _) = model.request(&options, true).unwrap();
    assert_eq!(
        request.url().as_str(),
        "https://example.test/v1/chat/completions"
    );
    assert!(request.headers()[AUTHORIZATION].is_sensitive());
    let body: Value = serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body["messages"][0],
        json!({"role":"system", "content":"规则"})
    );
    assert_eq!(
        body["messages"][1],
        json!({"role":"user", "content":"你好"})
    );
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn non_stream_response_maps_text_tool_calls_and_usage() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"gpt-test",
        "choices":[{"index":0,"message":{"role":"assistant","content":"你好","tool_calls":[{"id":"call_1","type":"function","function":{"name":"sum","arguments":"{\"a\":1}"}}]},"finish_reason":"tool_calls"}],
        "usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14,"prompt_tokens_details":{"cached_tokens":2}}
    });
    let result = response::parse_response("openai", value).unwrap();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(10));
    assert_eq!(result.usage.input_tokens.cache_read, Some(2));
    assert!(matches!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::ToolCalls
    ));
    assert!(result.content.iter().any(|item| matches!(item, zach_ai_core::OutputContent::ToolCall { tool_call_id, .. } if tool_call_id == "call_1")));
}

/// Ollama、vLLM 等本地端点不需要 Key，空 Key 不应被拒绝，也不应发送空的 Authorization。
#[test]
fn empty_api_key_sends_no_authorization_header() {
    let options = CallOptions::new(vec![Message::user("你好")]);
    let (request, _) = OpenAiChatCompletionsModel::new("", "llama3")
        .with_base_url("http://localhost:11434/v1")
        .request(&options, false)
        .unwrap();
    assert!(request.headers().get(AUTHORIZATION).is_none());
    assert_eq!(
        request.url().as_str(),
        "http://localhost:11434/v1/chat/completions"
    );
    assert!(OpenAiChatCompletionsModel::new(
        "bad
key", "llama3"
    )
    .request(&options, false)
    .is_err());
}

/// 非流式请求在生成完成前收不到任何字节，只能按整体时长设限；流式请求的总时长由生成长度
/// 决定，不能套用整体超时，而由驱动循环的空闲守卫约束。
#[test]
fn generate_requests_carry_a_total_timeout_and_stream_requests_do_not() {
    use std::time::Duration;
    let options = CallOptions::new(vec![Message::user("你好")]);
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    let (generate, _) = model.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), Some(&Duration::from_secs(600)));
    let (stream, _) = model.request(&options, true).unwrap();
    assert_eq!(stream.timeout(), None);
    let relaxed = model
        .clone()
        .with_generate_timeout(Some(Duration::from_secs(1800)));
    let (generate, _) = relaxed.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), Some(&Duration::from_secs(1800)));
    let unlimited = model.with_generate_timeout(None);
    let (generate, _) = unlimited.request(&options, false).unwrap();
    assert_eq!(generate.timeout(), None);
}

#[test]
fn chat_model_identity_does_not_expose_api_key() {
    let model = OpenAiChatCompletionsModel::new("private", "custom-model");
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), "custom-model");
    assert!(!format!("{model:?}").contains("private"));
}

/// 接入 DeepSeek、Qwen 等兼容端点时声明厂商身份：档案改按该厂商查内置目录，
/// 扩展参数改读该厂商键（未提供时回退 openai 键），回放标记仍是协议层的 openai 键。
#[test]
fn declared_provider_drives_catalog_lookup_and_provider_options_key() {
    use zach_ai_core::ProviderOptions;
    let catalog = crate::ModelCatalog::builtin();
    let deepseek = catalog.provider_models("deepseek")[0];
    let model = OpenAiChatCompletionsModel::new("k", &deepseek.id)
        .with_base_url("https://api.deepseek.com/v1")
        .with_provider("deepseek");
    assert_eq!(model.provider(), "deepseek");
    assert_eq!(model.profile(), Some(deepseek));
    assert!(OpenAiChatCompletionsModel::new("k", &deepseek.id)
        .profile()
        .is_none());
    let mut options = CallOptions::new(vec![Message::user("x")]);
    let mut provider = ProviderOptions::new();
    provider.insert("deepseek", json!({"thinking": {"type": "disabled"}}));
    provider.insert("openai", json!({"ignored": true}));
    options.provider_options = Some(provider);
    let (_, built) = model.request(&options, false).unwrap();
    assert_eq!(built.body["thinking"], json!({"type": "disabled"}));
    assert!(built.body.get("ignored").is_none());
    let mut provider = ProviderOptions::new();
    provider.insert("openai", json!({"enable_thinking": false}));
    options.provider_options = Some(provider);
    let (_, built) = model.request(&options, false).unwrap();
    assert_eq!(built.body["enable_thinking"], false);
}

/// 兼容端点（DeepSeek/Qwen）在 message.reasoning_content 返回思考链，不能被静默丢弃。
#[test]
fn non_stream_response_surfaces_reasoning_content() {
    let value = json!({
        "id":"chatcmpl-1", "object":"chat.completion", "created":1700000000, "model":"r1",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning_content":"先算加法","content":"结果是 3"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
    });
    let result = response::parse_response("openai", value).unwrap();
    assert_eq!(result.reasoning().as_deref(), Some("先算加法"));
    assert_eq!(result.text(), "结果是 3");
    assert!(matches!(
        result.content[0],
        zach_ai_core::OutputContent::Reasoning { .. }
    ));
    let via_reasoning = json!({
        "id":"chatcmpl-2", "object":"chat.completion", "created":1700000000, "model":"r1",
        "choices":[{"index":0,"message":{"role":"assistant","reasoning":"换个字段","content":"3"},"finish_reason":"stop"}]
    });
    let result = response::parse_response("openai", via_reasoning).unwrap();
    assert_eq!(result.reasoning().as_deref(), Some("换个字段"));
}

/// URL 支持声明必须与请求构建的实际能力一致，防止宿主跳过下载后构建失败。
#[test]
fn url_support_claims_match_request_builder_capabilities() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    assert!(model.is_url_supported("image/png", "https://example.com/a.png"));
    assert!(!model.is_url_supported("application/pdf", "https://example.com/a.pdf"));
    assert!(!model.is_url_supported("image/png", "file:///tmp/a.png"));
}

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
        request::build_request("gpt-test", "openai", None, &image, false),
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
    let body = request::build_request("gpt-test", "openai", None, &text, false)
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
    let built = request::build_request("qwen3", "alibaba", None, &options, false).unwrap();
    assert!(built.body.get("reasoning_effort").is_none());
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Compatibility { feature, details: Some(details) }
            if feature == "reasoning_effort.none" && details.contains("provider_options.alibaba")
    ));
    let built = request::build_request("gpt-5.1", "openai", None, &options, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "none");
    assert!(built.warnings.is_empty());
    // 其他档位仍按 reasoning_effort 发送，由服务端裁决。
    let high = CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::High);
    let built = request::build_request("qwen3", "alibaba", None, &high, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "high");
    assert!(built.warnings.is_empty());
}

/// OpenAI 官方端点只认 `max_completion_tokens`，第三方兼容端点普遍只认 `max_tokens`。
#[test]
fn output_limit_field_follows_the_declared_provider() {
    let options = CallOptions::new(vec![Message::user("x")]).with_max_output_tokens(64);
    let openai = request::build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(openai["max_completion_tokens"], 64);
    assert!(openai.get("max_tokens").is_none());
    let deepseek = request::build_request("deepseek-chat", "deepseek", None, &options, false)
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
    let body = request::build_request("gpt-test", "openai", None, &options, false)
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
    let body = request::build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
    assert!(body["response_format"]["json_schema"]
        .get("strict")
        .is_none());
    options.response_format = Some(ResponseFormat::json_schema(schema).with_strict(true));
    let body = request::build_request("gpt-test", "openai", None, &options, false)
        .unwrap()
        .body;
    assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    options.response_format = Some(ResponseFormat::Json {
        schema: None,
        name: None,
        description: None,
        strict: None,
    });
    let body = request::build_request("gpt-test", "openai", None, &options, false)
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
    let body = request::build_request("qwen", "openai", None, &options, false)
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
                request::build_request("qwen", "openai", None, &options, false),
                Err(zach_ai_core::ModelError::UnsupportedFeature { feature, .. }) if feature == key
            ),
            "{key} 不应被覆盖"
        );
    }
}

/// 注入的档案参与请求校验：不支持推理的模型丢弃档位并给出警告。
#[test]
fn injected_profile_drives_request_validation() {
    let gpt41 = crate::ModelCatalog::builtin()
        .get("openai", "gpt-4.1")
        .unwrap()
        .clone();
    let model = OpenAiChatCompletionsModel::new("k", "proxy-model").with_profile(gpt41);
    let options = CallOptions::new(vec![Message::user("x")])
        .with_reasoning(zach_ai_core::ReasoningEffort::High);
    let (_, built) = model.request(&options, false).unwrap();
    assert!(built.body.get("reasoning_effort").is_none());
    assert!(matches!(
        &built.warnings[0],
        zach_ai_core::ModelWarning::Unsupported { feature, .. } if feature == "reasoning"
    ));
}

/// `max` 档位由档案裁决而非一刀切拒绝：gpt-5.6 及更新代际照常发送，仅到 xhigh 的代际
/// 降级并给出警告，连 xhigh 都没有的代际发送前报错；档案未知时原样发送由服务端裁决。
#[test]
fn max_effort_follows_the_profile_instead_of_a_blanket_rejection() {
    let catalog = crate::ModelCatalog::builtin();
    let mut options = CallOptions::new(vec![Message::user("你好")]);
    options.reasoning = Some(zach_ai_core::ReasoningEffort::Max);
    let newest = catalog.get("openai", "gpt-5.6").unwrap();
    let built =
        request::build_request(&newest.id, "openai", Some(newest), &options, false).unwrap();
    assert_eq!(built.body["reasoning_effort"], "max");
    assert!(built.warnings.is_empty());
    let xhigh_only = catalog.get("openai", "gpt-5.2").unwrap();
    let built = request::build_request(&xhigh_only.id, "openai", Some(xhigh_only), &options, false)
        .unwrap();
    assert_eq!(built.body["reasoning_effort"], "xhigh");
    assert!(matches!(
        &built.warnings[0],
        zach_ai_core::ModelWarning::Compatibility { feature, .. }
            if feature == "reasoning_effort.max"
    ));
    let no_xhigh = catalog.get("openai", "gpt-5").unwrap();
    assert!(matches!(
        request::build_request(&no_xhigh.id, "openai", Some(no_xhigh), &options, false),
        Err(zach_ai_core::ModelError::UnsupportedFeature { feature, .. })
            if feature == "reasoning_effort.max"
    ));
    let unknown = request::build_request("proxy-model", "openai", None, &options, false).unwrap();
    assert_eq!(unknown.body["reasoning_effort"], "max");
}
