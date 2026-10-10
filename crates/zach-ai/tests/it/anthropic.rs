//! Anthropic Messages 模型公开 API 的构造、能力和请求前校验。

use zach_ai::{AnthropicMessagesModel, ModelCatalog};
use zach_ai_core::{CallOptions, LanguageModel, Message, ModelError};

#[test]
fn messages_model_exposes_identity_and_catalog_profile_without_credentials_in_debug() {
    let profile = ModelCatalog::builtin().provider_models("anthropic")[0];
    let model = AnthropicMessagesModel::new("private-api-key", &profile.id);
    assert_eq!(model.provider(), "anthropic");
    assert_eq!(model.model_id(), profile.id);
    assert_eq!(model.profile(), Some(profile));
    assert!(!format!("{model:?}").contains("private-api-key"));
    let custom = AnthropicMessagesModel::new("secret", "custom-unknown-model");
    assert!(custom.profile().is_none());
    assert!(custom.is_url_supported("image/png", "https://example.com/image.png"));
    assert!(custom.is_url_supported("application/pdf", "https://example.com/doc.pdf"));
    assert!(!custom.is_url_supported("audio/wav", "https://example.com/audio.wav"));
    assert!(!custom.is_url_supported("image/png", "file:///a.png"));
}

/// 自定义端点或目录未收录的模型可注入档案；注入值优先于内置目录中的同名条目。
#[test]
fn injected_profile_overrides_the_builtin_catalog() {
    use zach_ai_core::ModelProfile;
    let builtin = ModelCatalog::builtin().provider_models("anthropic")[0];
    let custom = ModelProfile::new("anthropic", &builtin.id, 1_000, 100);
    let model = AnthropicMessagesModel::new("secret", &builtin.id).with_profile(custom.clone());
    assert_eq!(model.profile(), Some(&custom));
    let unknown = AnthropicMessagesModel::new("secret", "proxy-model")
        .with_profile(ModelProfile::new("anthropic", "proxy-model", 8_000, 2_000));
    assert_eq!(unknown.profile().unwrap().limits.max_output_tokens, 2_000);
}

#[tokio::test]
async fn unsupported_options_fail_without_accessing_the_endpoint() {
    let model =
        AnthropicMessagesModel::new("secret", "example").with_base_url("https://unused.invalid");
    let options = CallOptions {
        prompt: vec![Message::user("你好")].into(),
        seed: Some(5),
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
    let late_system = CallOptions::new(vec![Message::user("你好"), Message::system("规则")]);
    assert!(matches!(
        model.do_generate(late_system).await,
        Err(ModelError::InvalidRequest(_))
    ));
}
