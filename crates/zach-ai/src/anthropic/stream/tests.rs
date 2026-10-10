//! 离线验证 Messages SSE 传输、内容块生命周期和多轮回放。

mod failures;
mod replay;

use super::*;
use futures::StreamExt;
use serde_json::{json, Value};
use zach_ai_core::{GenerateResult, ModelError, StreamAccumulator};

fn message(content: Vec<Value>, stop_reason: &str) -> Value {
    json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "example",
        "content": content, "stop_reason": stop_reason, "stop_sequence": null,
        "usage": {
            "input_tokens": 60, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 10,
            "output_tokens": 20
        }
    })
}

fn frame(value: &Value) -> String {
    format!(
        "event: {}\r\ndata: {value}\r\n\r\n",
        value["type"].as_str().unwrap()
    )
}

fn start_frame() -> String {
    frame(&json!({"type": "message_start", "message": {
        "id": "msg_1", "type": "message", "role": "assistant", "model": "example",
        "content": [], "stop_reason": null, "stop_sequence": null,
        "usage": {"input_tokens": 60, "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 10, "output_tokens": 1}
    }}))
}

fn stop_frames(stop_reason: &str) -> String {
    frame(
        &json!({"type": "message_delta", "delta": {"stop_reason": stop_reason, "stop_sequence": null},
        "usage": {"output_tokens": 20}}),
    ) + &frame(&json!({"type": "message_stop"}))
}

async fn parse_wire(wire: String) -> Vec<Result<StreamPart, ModelError>> {
    // 每次只交付一个字节，确保 UTF-8 字符和 CRLF 都会跨越传输边界。
    let chunks = wire
        .into_bytes()
        .into_iter()
        .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![b])))
        .collect::<Vec<_>>();
    messages_stream(
        futures::stream::iter(chunks),
        MessagesStreamParser::new(false),
        vec![],
        None,
    )
    .collect()
    .await
}

fn aggregate(parts: Vec<Result<StreamPart, ModelError>>) -> GenerateResult {
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    assert!(accumulator.is_complete());
    assert!(
        accumulator.errors().is_empty(),
        "{:?}",
        accumulator.errors()
    );
    accumulator.finish()
}

#[tokio::test]
async fn utf8_text_deltas_and_cumulative_usage_produce_one_text_block() {
    let mut wire = "\u{feff}: keep-alive\r\n\r\n".to_owned() + &start_frame();
    for event in [
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "ping"}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "你"}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "好"}}),
        json!({"type": "content_block_stop", "index": 0}),
    ] {
        wire.push_str(&frame(&event));
    }
    wire.push_str(&stop_frames("end_turn"));
    let parts = parse_wire(wire).await;
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::TextStart { .. })))
            .count(),
        1
    );
    assert_eq!(
        parts
            .iter()
            .filter(|p| matches!(p, Ok(StreamPart::TextEnd { .. })))
            .count(),
        1
    );
    let result = aggregate(parts);
    assert_eq!(result.text(), "你好");
    assert_eq!(result.usage.input_tokens.total, Some(100));
    assert_eq!(result.usage.input_tokens.no_cache, Some(60));
    assert_eq!(result.usage.input_tokens.cache_read, Some(30));
    assert_eq!(result.usage.input_tokens.cache_write, Some(10));
    assert_eq!(result.usage.output_tokens.total, Some(20));
    assert_eq!(
        result.finish_reason.unified,
        zach_ai_core::UnifiedFinishReason::Stop
    );
    let response = result.response.unwrap();
    assert_eq!(response.id.as_deref(), Some("msg_1"));
    assert_eq!(response.model_id.as_deref(), Some("example"));
}

#[tokio::test]
async fn message_stop_ends_the_stream_without_waiting_for_network_eof() {
    let wire = start_frame()
        + &frame(&json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": "结束"}}))
        + &frame(&json!({"type": "content_block_stop", "index": 0}))
        + &stop_frames("end_turn");
    let source = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(wire))]).chain(
        futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
                panic!("message_stop 之后不应再读取源流")
            },
        ),
    );
    let result = aggregate(
        messages_stream(source, MessagesStreamParser::new(false), vec![], None)
            .collect()
            .await,
    );
    assert_eq!(result.text(), "结束");
}

#[tokio::test]
async fn stream_is_lazy_and_dropping_it_releases_the_source() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct Probe(Arc<AtomicBool>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let probe = Probe(dropped.clone());
    let source = futures::stream::poll_fn(
        move |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
            let _ = &probe;
            panic!("只取 StreamStart 时不应拉取网络")
        },
    );
    let mut stream = messages_stream(source, MessagesStreamParser::new(false), vec![], None);
    assert!(matches!(
        stream.next().await,
        Some(Ok(StreamPart::StreamStart { .. }))
    ));
    drop(stream);
    assert!(dropped.load(Ordering::SeqCst));
}
