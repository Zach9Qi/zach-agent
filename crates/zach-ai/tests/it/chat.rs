//! Chat Completions 模型公开 API 的构造、能力和请求前校验。

use zach_ai::{ModelCatalog, OpenAiChatCompletionsModel};
use zach_ai_core::{CallOptions, LanguageModel, Message, ModelError, ModelProfile};

#[test]
fn chat_model_exposes_identity_and_catalog_profile_without_credentials_in_debug() {
    let profile = ModelCatalog::builtin().provider_models("openai")[0];
    let model = OpenAiChatCompletionsModel::new("private-api-key", &profile.id);
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), profile.id);
    assert_eq!(model.profile(), Some(profile));
    assert!(!format!("{model:?}").contains("private-api-key"));
    // 兼容端点上的模型不在内置目录中，可注入档案。
    let injected = ModelProfile::new("openai", "local-llm", 32_768, 4_096);
    let local = OpenAiChatCompletionsModel::new("", "local-llm")
        .with_base_url("http://localhost:11434/v1")
        .with_profile(injected.clone());
    assert_eq!(local.profile(), Some(&injected));
    assert!(OpenAiChatCompletionsModel::new("k", "unknown")
        .profile()
        .is_none());
}

/// URL 支持声明必须与请求构建能力一致：图片 URL 直传，PDF 只接受 file_id 或内联数据。
#[test]
fn url_support_claims_match_request_builder_capabilities() {
    let model = OpenAiChatCompletionsModel::new("secret", "gpt-test");
    assert!(model.is_url_supported("image/png", "https://example.com/a.png"));
    assert!(!model.is_url_supported("application/pdf", "https://example.com/a.pdf"));
    assert!(!model.is_url_supported("image/png", "file:///tmp/a.png"));
}

#[tokio::test]
async fn unsupported_options_fail_without_accessing_the_endpoint() {
    let model = OpenAiChatCompletionsModel::new("secret", "example")
        .with_base_url("https://unused.invalid/v1");
    let options = CallOptions {
        prompt: vec![Message::user("你好")].into(),
        top_k: Some(5),
        ..Default::default()
    };
    assert!(matches!(
        model.do_generate(options.clone()).await,
        Err(ModelError::UnsupportedFeature { .. })
    ));
    assert!(matches!(
        model.do_stream(options).await,
        Err(ModelError::UnsupportedFeature { .. })
    ));
}
