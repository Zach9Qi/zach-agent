//! Anthropic Messages 模型公开 API 的构造、能力、请求前校验与错误分类。

use futures::StreamExt;
use zach_ai::{AnthropicMessagesModel, ModelCatalog};
use zach_ai_core::{CallOptions, ErrorKind, LanguageModel, Message, StreamPart};

use crate::support::{serve, Body, Script};

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
    assert!(model
        .do_generate(options.clone())
        .await
        .is_err_and(|e| e.kind() == ErrorKind::Unsupported));
    assert!(model
        .do_stream(options)
        .await
        .is_err_and(|e| e.kind() == ErrorKind::Unsupported));
}

/// 过载既可能是 HTTP 529，也可能是 HTTP 200 之后的流内 error 事件；两条路径必须得到
/// 同一个可重试类别，否则同一个故障一种会重试、一种直接让运行失败
#[tokio::test]
async fn overload_is_retryable_whether_it_arrives_as_http_529_or_as_a_stream_event() {
    let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
    let options = CallOptions::new(vec![Message::user("你好")]).with_max_output_tokens(16);

    let (base, _) = serve(Script {
        status: 529,
        content_type: "application/json",
        body: Body::Full(body.to_owned()),
    })
    .await;
    let model = AnthropicMessagesModel::new("secret", "example").with_base_url(base);
    let http = model.do_generate(options.clone()).await.unwrap_err();
    assert_eq!(http.kind(), ErrorKind::Unavailable);
    assert_eq!(http.status(), Some(529));
    assert!(http.is_retryable());

    let (base, _) = serve(Script {
        status: 200,
        content_type: "text/event-stream",
        body: Body::Chunked {
            chunks: vec![format!("event: error\ndata: {body}\n\n")],
            complete: true,
        },
    })
    .await;
    let model = AnthropicMessagesModel::new("secret", "example").with_base_url(base);
    let parts: Vec<_> = model.do_stream(options).await.unwrap().collect().await;
    let event = parts
        .iter()
        .find_map(|part| match part {
            Ok(StreamPart::Error(error)) => Some(error),
            _ => None,
        })
        .expect("应透出流内错误事件");
    assert_eq!(event.kind, http.kind());
    assert_eq!(event.code.as_deref(), http.code());
}
