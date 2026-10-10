//! Responses 模型公开 API 的构造、能力和请求前校验。

use zach_ai::{ModelCatalog, OpenAiResponsesModel};
use zach_ai_core::{CallOptions, LanguageModel, ModelError};

#[test]
fn responses_model_exposes_identity_and_catalog_profile_without_credentials_in_debug() {
    let profile = ModelCatalog::builtin().provider_models("openai")[0];
    let model = OpenAiResponsesModel::new("private-api-key", &profile.id);
    assert_eq!(model.provider(), "openai");
    assert_eq!(model.model_id(), profile.id);
    assert_eq!(model.profile(), Some(profile));
    assert!(!format!("{model:?}").contains("private-api-key"));
    let custom = OpenAiResponsesModel::new("secret", "custom-unknown-model");
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
    let builtin = ModelCatalog::builtin().provider_models("openai")[0];
    let custom = ModelProfile::new("openai", &builtin.id, 1_000, 100);
    let model = OpenAiResponsesModel::new("secret", &builtin.id).with_profile(custom.clone());
    assert_eq!(model.profile(), Some(&custom));
}

#[tokio::test]
async fn unsupported_options_fail_without_accessing_the_endpoint() {
    let model =
        OpenAiResponsesModel::new("secret", "example").with_base_url("https://unused.invalid/v1");
    let options = CallOptions {
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
