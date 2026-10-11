//! 传输层对真实 HTTP 行为的处理：状态码分类、流式请求收到非流式正文、分块 SSE 的解码、
//! 中途断开，以及非流式 JSON 响应与头的读取。以 Chat Completions 适配器为载体。

use futures::StreamExt;
use serde_json::Value;
use zach_ai::OpenAiChatCompletionsModel;
use zach_ai_core::{
    CallOptions, ErrorKind, LanguageModel, Message, StreamAccumulator, StreamPart,
    UnifiedFinishReason,
};

use crate::support::{serve, Body, Script};

fn model(base: &str) -> OpenAiChatCompletionsModel {
    OpenAiChatCompletionsModel::new("secret", "mock-model").with_base_url(format!("{base}/v1"))
}

fn options() -> CallOptions {
    CallOptions::new(vec![Message::user("你好")])
}

fn json(status: u16, body: &str) -> Script {
    Script {
        status,
        content_type: "application/json",
        body: Body::Full(body.to_owned()),
    }
}

fn sse(chunks: Vec<String>, complete: bool) -> Script {
    Script {
        status: 200,
        content_type: "text/event-stream",
        body: Body::Chunked { chunks, complete },
    }
}

fn chunk(delta: &str, finish: Option<&str>) -> String {
    format!(
        "data: {{\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\
         \"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{}}}]}}\n\n",
        finish.map_or("null".to_owned(), |reason| format!("\"{reason}\""))
    )
}

#[tokio::test]
async fn http_statuses_map_to_stable_error_kinds() {
    for (status, kind) in [
        (401, ErrorKind::Authentication),
        (429, ErrorKind::RateLimited),
        (503, ErrorKind::Unavailable),
        (400, ErrorKind::Rejected),
    ] {
        let (base, _) = serve(json(status, r#"{"error":{"message":"nope"}}"#)).await;
        let error = model(&base).do_generate(options()).await.unwrap_err();
        assert_eq!(error.kind(), kind, "HTTP {status}: {error:?}");
        assert_eq!(error.status(), Some(status));
        assert_eq!(error.provider(), Some("openai"));
        assert_eq!(error.message(), "nope");
        assert!(error.raw().is_some());
    }
}

/// 状态码同为 429：限流可重试，配额耗尽需要充值、不可重试；上下文超长单独成类，便于压缩后重试
#[tokio::test]
async fn openai_error_codes_refine_the_status_based_kind() {
    for (status, code, kind) in [
        (429, "rate_limit_exceeded", ErrorKind::RateLimited),
        (429, "insufficient_quota", ErrorKind::Rejected),
        (
            400,
            "context_length_exceeded",
            ErrorKind::ContextLengthExceeded,
        ),
        (401, "invalid_api_key", ErrorKind::Authentication),
    ] {
        let body = format!(r#"{{"error":{{"message":"m","type":"t","code":"{code}"}}}}"#);
        let (base, _) = serve(json(status, &body)).await;
        let error = model(&base).do_generate(options()).await.unwrap_err();
        assert_eq!(error.kind(), kind, "{code}");
        assert_eq!(error.code(), Some(code));
    }
}

/// 部分兼容端点以 HTTP 200 返回错误对象：按厂商错误分类，而不是报"缺少 choices"
#[tokio::test]
async fn generate_answered_with_an_error_object_is_classified_as_a_provider_error() {
    let body = r#"{"error":{"message":"busy","code":"server_error"}}"#;
    let (base, _) = serve(json(200, body)).await;
    let error = model(&base).do_generate(options()).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Unavailable);
    assert_eq!(error.message(), "busy");
}

/// 网关返回 HTML 错误页：报告为响应无法解析，并保留正文开头便于定位
#[tokio::test]
async fn generate_answered_with_html_keeps_a_body_snippet() {
    let (base, _) = serve(Script {
        status: 200,
        content_type: "text/html",
        body: Body::Full("<html>bad gateway</html>".into()),
    })
    .await;
    let error = model(&base).do_generate(options()).await.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidResponse);
    assert_eq!(
        error.raw().and_then(Value::as_str),
        Some("<html>bad gateway</html>")
    );
    assert!(std::error::Error::source(&error).is_some());
}

/// 网关把流式请求当普通请求处理时回 JSON：正文里的错误信息必须保留，而不是只报内容类型不对。
#[tokio::test]
async fn stream_answered_with_json_is_a_protocol_error_carrying_the_body_message() {
    let (base, _) = serve(json(200, r#"{"error":{"message":"streaming disabled"}}"#)).await;
    let Err(error) = model(&base).do_stream(options()).await else {
        panic!("非流式正文不应建立成流");
    };
    assert_eq!(error.kind(), ErrorKind::InvalidResponse, "{error:?}");
    assert!(error.message().contains("streaming disabled"), "{error:?}");
}

#[tokio::test]
async fn chunked_sse_is_decoded_aggregated_and_sent_with_expected_headers() {
    let first = chunk(r#"{"role":"assistant","content":"你"}"#, None);
    let second = chunk(r#"{"content":"好"}"#, Some("stop"));
    let tail = "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"m\",\
                \"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                data: [DONE]\n\n";
    let (base, request) = serve(sse(vec![first, second, tail.to_owned()], true)).await;
    let mut stream = model(&base).do_stream(options()).await.unwrap();
    let mut accumulator = StreamAccumulator::new();
    while let Some(part) = stream.next().await {
        accumulator.process(part.unwrap());
    }
    let result = accumulator.finish();
    assert_eq!(result.text(), "你好");
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::Stop);
    assert_eq!(result.usage.input_tokens.total, Some(3));
    let request = request.await.unwrap();
    assert_eq!(request.header("authorization"), Some("Bearer secret"));
    assert_eq!(request.header("accept"), Some("text/event-stream"));
    assert!(request.head[0].starts_with("POST /v1/chat/completions "));
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["model"], "mock-model");
}

/// 服务端在 `finish_reason` 之前断开（分块未终止）是传输错误，已到的正文仍先透出。
#[tokio::test]
async fn connection_closed_mid_stream_is_a_transport_error() {
    let half = chunk(r#"{"content":"半"}"#, None);
    let (base, _) = serve(sse(vec![half], false)).await;
    let parts: Vec<_> = model(&base)
        .do_stream(options())
        .await
        .unwrap()
        .collect()
        .await;
    assert!(
        parts
            .iter()
            .any(|p| matches!(p, Ok(StreamPart::TextDelta { delta, .. }) if delta == "半")),
        "{parts:?}"
    );
    assert!(
        matches!(parts.last(), Some(Err(error)) if error.kind() == ErrorKind::Transport),
        "{parts:?}"
    );
}

#[tokio::test]
async fn generate_reads_the_json_body_and_keeps_response_headers() {
    let body = r#"{"id":"c1","object":"chat.completion","created":1,"model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    let (base, request) = serve(json(200, body)).await;
    let result = model(&base).do_generate(options()).await.unwrap();
    assert_eq!(result.text(), "ok");
    assert_eq!(result.usage.output_tokens.total, Some(1));
    assert_eq!(
        result
            .response_headers
            .as_ref()
            .and_then(|headers| headers.get("x-test"))
            .map(String::as_str),
        Some("mock")
    );
    assert_eq!(result.request_body.as_ref().unwrap()["model"], "mock-model");
    assert_eq!(
        request.await.unwrap().header("accept"),
        Some("application/json")
    );
}
