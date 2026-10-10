//! 离线验证 Responses SSE 传输、语义事件和多轮回放。

mod failures;
mod replay;

use super::*;
use futures::StreamExt;
use serde_json::{json, Value};
use zach_ai_core::{GenerateResult, ModelError, StreamAccumulator};

fn response(output: Vec<Value>) -> Value {
    json!({
        "id": "resp_1", "model": "example", "status": "completed", "created_at": 1700000000,
        "output": output,
        "usage": {
            "input_tokens": 100, "input_tokens_details": {"cached_tokens": 40},
            "output_tokens": 20, "output_tokens_details": {"reasoning_tokens": 8}
        }
    })
}

fn message(text: &str) -> Value {
    json!({"type": "message", "id": "msg_1", "role": "assistant", "status": "completed",
        "content": [{"type": "output_text", "text": text, "annotations": []}]})
}

fn frame(value: &Value) -> String {
    format!(
        "event: {}\r\ndata: {value}\r\n\r\n",
        value["type"].as_str().unwrap()
    )
}

async fn parse_wire(wire: String) -> Vec<Result<StreamPart, ModelError>> {
    // 每次只交付一个字节，确保 UTF-8 字符和 CRLF 都会跨越传输边界。
    let chunks = wire
        .into_bytes()
        .into_iter()
        .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![b])))
        .collect::<Vec<_>>();
    responses_stream(
        futures::stream::iter(chunks),
        ResponsesStreamParser::new(false),
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
async fn utf8_deltas_and_final_snapshots_produce_text_once() {
    let mut wire = "\u{feff}: keep-alive\r\n\r\n".to_owned();
    for event in [
        json!({"type": "response.output_item.added", "item": {
            "type": "message", "id": "msg_1", "role": "assistant", "content": []
        }}),
        json!({"type": "response.content_part.added", "item_id": "msg_1",
            "content_index": 0, "part": {"type": "output_text", "text": ""}}),
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "content_index": 0, "delta": "你"}),
        json!({"type": "response.output_text.delta", "item_id": "msg_1", "content_index": 0, "delta": "好"}),
        json!({"type": "response.output_text.done", "item_id": "msg_1", "content_index": 0, "text": "你好"}),
        json!({"type": "response.content_part.done", "item_id": "msg_1", "content_index": 0,
            "part": {"type": "output_text", "text": "你好", "annotations": []}}),
        json!({"type": "response.output_item.done", "item": message("你好")}),
        json!({"type": "response.completed", "response": response(vec![message("你好")])}),
    ] {
        wire.push_str(&frame(&event));
    }
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
    assert_eq!(result.usage.input_tokens.no_cache, Some(60));
    assert_eq!(result.usage.output_tokens.reasoning, Some(8));
    assert_eq!(result.response.unwrap().timestamp, Some(1700000000000));
}

#[tokio::test]
async fn completed_response_stops_without_waiting_for_network_eof() {
    let wire =
        frame(&json!({"type": "response.completed", "response": response(vec![message("结束")])}));
    let source = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(wire))]).chain(
        futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
                panic!("语义终止后不应再读取源流")
            },
        ),
    );
    let result = aggregate(
        responses_stream(source, ResponsesStreamParser::new(false), vec![], None)
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
    let mut stream = responses_stream(source, ResponsesStreamParser::new(false), vec![], None);
    assert!(matches!(
        stream.next().await,
        Some(Ok(StreamPart::StreamStart { .. }))
    ));
    drop(stream);
    assert!(dropped.load(Ordering::SeqCst));
}
