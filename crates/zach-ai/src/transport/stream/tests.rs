//! 驱动循环的终止语义：终止事件、错误事件、连接断开与空闲超时的组合。

use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 脚本化解析器：`done` 帧置位 terminal，`fail` 帧发 Error 并置位 failed，
/// `finish` 帧发 Finish 并置位 finished，其余原样透出。
struct Scripted {
    terminal: bool,
    failed: bool,
    finished: bool,
}

impl SseParser for Scripted {
    fn data(&mut self, data: &str) -> Vec<StreamPart> {
        match data {
            "done" => {
                self.terminal = true;
                vec![]
            }
            "fail" => {
                self.failed = true;
                vec![StreamPart::Error {
                    message: "服务端错误".into(),
                    raw: None,
                }]
            }
            "finish" => {
                self.finished = true;
                vec![StreamPart::Finish {
                    usage: Default::default(),
                    finish_reason: zach_ai_core::FinishReason::stop(),
                    provider_metadata: None,
                }]
            }
            other => vec![StreamPart::TextDelta {
                id: "t".into(),
                delta: other.into(),
                provider_metadata: None,
            }],
        }
    }
    fn terminal(&self) -> bool {
        self.terminal
    }
    fn failed(&self) -> bool {
        self.failed
    }
    fn finished(&self) -> bool {
        self.finished
    }
}

fn parser() -> Scripted {
    Scripted {
        terminal: false,
        failed: false,
        finished: false,
    }
}

async fn run(
    chunks: Vec<Result<&'static str, std::io::Error>>,
) -> Vec<Result<StreamPart, ModelError>> {
    let source = futures::stream::iter(chunks.into_iter().map(|chunk| chunk.map(Bytes::from)));
    sse_stream("测试", source, parser(), vec![], None)
        .collect()
        .await
}

#[tokio::test]
async fn eof_before_terminal_without_error_event_is_a_transport_error() {
    let parts = run(vec![Ok("data: a\n\n")]).await;
    assert!(matches!(&parts[0], Ok(StreamPart::StreamStart { .. })));
    assert!(matches!(&parts[1], Ok(StreamPart::TextDelta { delta, .. }) if delta == "a"));
    assert!(matches!(
        &parts[2],
        Err(ModelError::StreamError { message, .. }) if message.contains("终止事件")
    ));
}

/// 服务端在流中报错后直接断开是常态，无论是干净 FIN 还是 RST，
/// 都不能再叠加一个可重试的传输错误。
#[tokio::test]
async fn connection_loss_after_error_event_is_not_reported_again() {
    let parts = run(vec![Ok("data: fail\n\n")]).await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[1], Ok(StreamPart::Error { .. })));
    let parts = run(vec![
        Ok("data: fail\n\n"),
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
    ])
    .await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(&parts[1], Ok(StreamPart::Error { .. })));
}

/// `Finish` 已发出后响应即语义完整，终止符之前的断开（FIN 或 RST）不改变结果。
#[tokio::test]
async fn connection_loss_after_finish_keeps_the_response_successful() {
    for tail in [
        vec![],
        vec![Err(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset,
        ))],
    ] {
        let mut chunks = vec![Ok("data: a\n\ndata: finish\n\n")];
        chunks.extend(tail);
        let parts = run(chunks).await;
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(parts.iter().all(Result::is_ok));
        assert!(matches!(&parts[2], Ok(StreamPart::Finish { .. })));
    }
}

#[tokio::test]
async fn terminal_frame_stops_reading_even_if_more_bytes_follow() {
    let source = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
        "data: done\n\ndata: late\n\n",
    ))])
    .chain(futures::stream::poll_fn(
        |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
            panic!("终止后不应再读取源流")
        },
    ));
    let parts: Vec<_> = sse_stream("测试", source, parser(), vec![], None)
        .collect()
        .await;
    assert_eq!(parts.len(), 1);
}

#[tokio::test]
async fn transport_errors_end_the_stream_immediately() {
    let parts = run(vec![Err(std::io::Error::other("断开")), Ok("data: a\n\n")]).await;
    assert_eq!(parts.len(), 2);
    assert!(matches!(
        &parts[1],
        Err(ModelError::StreamError {
            source: Some(_),
            ..
        })
    ));
}

#[tokio::test]
async fn stream_is_lazy_and_dropping_it_releases_the_source() {
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
    let mut stream = sse_stream("测试", source, parser(), vec![], None);
    assert!(matches!(
        stream.next().await,
        Some(Ok(StreamPart::StreamStart { .. }))
    ));
    drop(stream);
    assert!(dropped.load(Ordering::SeqCst));
}

/// 相邻两次收到数据的间隔超过上限时按传输错误结束，计时在每次收到数据后重置。
#[tokio::test(start_paused = true)]
async fn idle_timeout_before_finish_is_a_transport_error_and_resets_per_chunk() {
    let paced = futures::stream::unfold(0u8, |sent| async move {
        if sent == 3 {
            return None;
        }
        tokio::time::sleep(Duration::from_secs(200)).await;
        Some((
            Ok::<_, std::io::Error>(Bytes::from("data: a\n\n")),
            sent + 1,
        ))
    })
    .chain(futures::stream::pending());
    let parts: Vec<_> = sse_stream(
        "测试",
        paced,
        parser(),
        vec![],
        Some(Duration::from_secs(300)),
    )
    .collect()
    .await;
    assert_eq!(parts.len(), 5, "{parts:?}");
    assert!(parts[1..4]
        .iter()
        .all(|p| matches!(p, Ok(StreamPart::TextDelta { .. }))));
    assert!(matches!(
        &parts[4],
        Err(ModelError::StreamError { message, source: None }) if message.contains("300 秒")
    ));
}

/// `Finish` 或错误事件之后的静默只是连接没有及时关闭，不改变结果。
#[tokio::test(start_paused = true)]
async fn idle_after_finish_or_error_ends_the_stream_quietly() {
    for (frame, expect_error) in [("data: finish\n\n", false), ("data: fail\n\n", true)] {
        let source = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(frame))])
            .chain(futures::stream::pending());
        let parts: Vec<_> = sse_stream(
            "测试",
            source,
            parser(),
            vec![],
            Some(Duration::from_secs(300)),
        )
        .collect()
        .await;
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(
            matches!(&parts[1], Ok(StreamPart::Error { .. })),
            expect_error
        );
    }
}
