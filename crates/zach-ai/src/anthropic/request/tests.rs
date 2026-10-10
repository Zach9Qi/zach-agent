//! Messages 请求映射：参数、系统提示、工具声明、推理档位与厂商扩展。

mod history;

use super::*;
use crate::test_support::{profile, reasoning_profile};
use serde_json::json;
use zach_ai_core::{FunctionTool, Message, ProviderTool, ToolChoice, ToolDefinition};

fn build(options: &CallOptions) -> Value {
    build_request("example", None, options, false).unwrap().body
}

fn anthropic_options(value: Value) -> Option<ProviderOptions> {
    let mut options = ProviderOptions::new();
    options.insert("anthropic", value);
    Some(options)
}

#[test]
fn request_requires_max_tokens_and_maps_sampling_parameters() {
    let options = CallOptions {
        prompt: vec![Message::user("你好")].into(),
        temperature: Some(0.5),
        top_p: Some(0.75),
        top_k: Some(40),
        stop_sequences: Some(vec!["END".into()]),
        ..Default::default()
    };
    let body = build(&options);
    assert_eq!(body["model"], "example");
    assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
    assert_eq!(body["stream"], false);
    assert_eq!(body["temperature"], 0.5);
    assert_eq!(body["top_p"], 0.75);
    assert_eq!(body["top_k"], 40);
    assert_eq!(body["stop_sequences"], json!(["END"]));
    assert!(body.get("system").is_none());
    assert_eq!(
        body["messages"],
        json!([{"role": "user", "content": [{"type": "text", "text": "你好"}]}])
    );
    let explicit = CallOptions::new(vec![Message::user("你好")]).with_max_output_tokens(321);
    assert_eq!(build(&explicit)["max_tokens"], 321);
}

#[test]
fn known_models_default_max_tokens_from_the_catalog() {
    let profile = crate::ModelCatalog::builtin().provider_models("anthropic")[0];
    let body = build_request(
        &profile.id,
        Some(profile),
        &CallOptions::new(vec![Message::user("你好")]),
        true,
    )
    .unwrap()
    .body;
    assert_eq!(body["max_tokens"], profile.limits.max_output_tokens);
    assert_eq!(body["stream"], true);
}

#[test]
fn unsupported_generic_parameters_fail_before_sending() {
    for options in [
        CallOptions {
            presence_penalty: Some(0.1),
            ..Default::default()
        },
        CallOptions {
            frequency_penalty: Some(0.1),
            ..Default::default()
        },
        CallOptions {
            seed: Some(7),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            build_request("example", None, &options, false),
            Err(ModelError::UnsupportedFeature { .. })
        ));
    }
    let nan = CallOptions {
        temperature: Some(f32::NAN),
        ..Default::default()
    };
    assert!(matches!(
        build_request("example", None, &nan, false),
        Err(ModelError::InvalidRequest(_))
    ));
    assert!(build_request(" ", None, &CallOptions::default(), false).is_err());
}

#[test]
fn system_messages_are_hoisted_and_only_allowed_at_the_start() {
    let options = CallOptions::new(vec![
        Message::System {
            content: "规则".into(),
            provider_options: anthropic_options(json!({"cache_control": {"type": "ephemeral"}})),
        },
        Message::system("补充"),
        Message::user("你好"),
    ]);
    let body = build(&options);
    assert_eq!(
        body["system"],
        json!([
            {"type": "text", "text": "规则", "cache_control": {"type": "ephemeral"}},
            {"type": "text", "text": "补充"}
        ])
    );
    let late = CallOptions::new(vec![Message::user("你好"), Message::system("规则")]);
    assert!(matches!(
        build_request("example", None, &late, false),
        Err(ModelError::InvalidRequest(_))
    ));
}

#[test]
fn reasoning_effort_maps_to_adaptive_thinking_and_effort() {
    let options = CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::High);
    let body = build(&options);
    assert_eq!(
        body["thinking"],
        json!({"type": "adaptive", "display": "summarized"})
    );
    assert_eq!(body["output_config"], json!({"effort": "high"}));
    let off = CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::None);
    assert_eq!(build(&off)["thinking"], json!({"type": "disabled"}));
    let default =
        CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::ProviderDefault);
    assert!(build(&default).get("thinking").is_none());
    // 显式厂商配置整体替换通用档位：派生的 effort 不残留，output_config 其余字段照常合并。
    let mut explicit =
        CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::Medium);
    explicit.provider_options = anthropic_options(json!({
        "thinking": {"type": "enabled", "budget_tokens": 2048},
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}},
        "betas": ["example-beta"],
        "metadata": {"user_id": "u1"}
    }));
    let built = build_request("example", None, &explicit, false).unwrap();
    assert_eq!(
        built.body["thinking"],
        json!({"type": "enabled", "budget_tokens": 2048})
    );
    assert!(built.body["output_config"].get("effort").is_none());
    assert_eq!(built.body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(built.body["metadata"], json!({"user_id": "u1"}));
    assert_eq!(built.betas, vec!["example-beta".to_owned()]);
}

/// 档案已知时按其校验：minimal 降级为 low 并给出警告，未声明可关闭推理时不发 `thinking`，
/// 不接受 temperature 的模型丢弃该参数；档案未知时原样发送。
#[test]
fn profile_downgrades_or_drops_settings_the_model_cannot_honor() {
    use zach_ai_core::ModelWarning;
    let fable = profile(
        "anthropic",
        "claude-fable-5",
        reasoning_profile(&[ReasoningEffort::Low, ReasoningEffort::High], false),
        false,
    );
    let mut options = CallOptions::new(vec![Message::user("x")])
        .with_reasoning(ReasoningEffort::Minimal)
        .with_temperature(0.5);
    let built = build_request(&fable.id, Some(&fable), &options, false).unwrap();
    assert_eq!(built.body["output_config"]["effort"], "low");
    assert!(built.body.get("temperature").is_none());
    assert_eq!(built.warnings.len(), 2);
    assert!(matches!(
        &built.warnings[0],
        ModelWarning::Compatibility { feature, .. } if feature == "reasoning_effort.minimal"
    ));
    assert!(matches!(
        &built.warnings[1],
        ModelWarning::Unsupported { feature, .. } if feature == "temperature"
    ));
    options.reasoning = Some(ReasoningEffort::None);
    options.temperature = None;
    let built = build_request(&fable.id, Some(&fable), &options, false).unwrap();
    assert!(built.body.get("thinking").is_none());
    assert_eq!(built.warnings.len(), 1);
    let built = build_request("custom", None, &options, false).unwrap();
    assert_eq!(built.body["thinking"], json!({"type": "disabled"}));
    assert!(built.warnings.is_empty());
}

/// Claude 4.6 之前的代际只认 `budget_tokens`，按 id 自动换算；思考开启后与之互斥的
/// `temperature` / `top_k` 被丢弃并给出警告，显式关闭思考时则保留。
#[test]
fn legacy_generations_use_budget_tokens_and_thinking_drops_sampling_parameters() {
    let sonnet = profile(
        "anthropic",
        "claude-sonnet-4-5",
        reasoning_profile(&[], false),
        true,
    );
    let mut options =
        CallOptions::new(vec![Message::user("x")]).with_reasoning(ReasoningEffort::High);
    options.temperature = Some(0.5);
    options.top_k = Some(40);
    options.top_p = Some(0.75);
    let built = build_request(&sonnet.id, Some(&sonnet), &options, false).unwrap();
    // 预算换算与采样参数丢弃的具体数值由 `thinking.rs` 的单元测试覆盖，这里只验证接线与顺序。
    assert_eq!(built.body["thinking"]["type"], "enabled");
    assert!(built.body.get("output_config").is_none());
    assert!(built.body.get("temperature").is_none());
    assert!(built.body.get("top_k").is_none());
    assert_eq!(built.body["top_p"], 0.75);
    assert_eq!(built.warnings.len(), 2);
    // 新代际走自适应思考；显式覆盖为关闭时采样参数保留。
    let built = build_request("claude-opus-4-6", None, &options, false).unwrap();
    assert_eq!(built.body["thinking"]["type"], "adaptive");
    assert_eq!(built.body["output_config"]["effort"], "high");
    options.provider_options = anthropic_options(json!({"thinking": {"type": "disabled"}}));
    let built = build_request("claude-opus-4-6", None, &options, false).unwrap();
    assert_eq!(built.body["temperature"], 0.5);
    assert_eq!(built.body["top_k"], 40);
    assert!(built.body.get("output_config").is_none());
    assert!(built.warnings.is_empty());
    // 覆盖使通用档位整体失效：档案不支持的档位不再触发发送前报错；去掉覆盖则照常拒绝。
    let opus = profile(
        "anthropic",
        "claude-opus-4-6",
        reasoning_profile(&[ReasoningEffort::High], false),
        true,
    );
    options.reasoning = Some(ReasoningEffort::Xhigh);
    assert!(build_request(&opus.id, Some(&opus), &options, false).is_ok());
    options.provider_options = None;
    assert!(build_request(&opus.id, Some(&opus), &options, false).is_err());
}

#[test]
fn json_schema_output_and_unknown_provider_fields_are_handled() {
    let mut options = CallOptions::new(vec![Message::user("x")]);
    options.response_format = Some(ResponseFormat::json_schema(json!({"type": "object"})));
    assert_eq!(
        build(&options)["output_config"]["format"],
        json!({"type": "json_schema", "schema": {"type": "object"}})
    );
    options.response_format = Some(ResponseFormat::Json {
        schema: None,
        name: None,
        description: None,
        strict: None,
    });
    assert!(matches!(
        build_request("example", None, &options, false),
        Err(ModelError::UnsupportedFeature { .. })
    ));
    options.response_format = Some(ResponseFormat::Text);
    assert!(build(&options).get("output_config").is_none());
    options.provider_options = anthropic_options(json!({"messages": []}));
    assert!(matches!(
        build_request("example", None, &options, false),
        Err(ModelError::UnsupportedFeature { .. })
    ));
    options.provider_options = anthropic_options(json!({"betas": "not-a-list"}));
    assert!(matches!(
        build_request("example", None, &options, false),
        Err(ModelError::InvalidRequest(_))
    ));
}

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
