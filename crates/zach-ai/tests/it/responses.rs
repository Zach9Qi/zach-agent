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
