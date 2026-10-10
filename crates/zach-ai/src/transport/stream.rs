//! 字节流 → SSE 帧 → 核心流事件的驱动循环。
//!
//! 流按消费需求拉取：消费方丢弃流即停止读取网络。三家适配器的终止语义统一为：
//! - 解析器报告 `terminal` 后不再读取源流；
//! - 源流在 `terminal` 之前结束（干净 EOF 或传输错误）时，只有解析器既没报告过
//!   错误事件、也没发出过 `Finish`，才视为传输错误；
//!   - 已报告错误事件：服务端通常随后直接断开（FIN 或 RST 都有），再叠加一个可重试的
//!     传输错误会让上层把不可重试的厂商错误误判为传输故障并反复重试；
//!   - 已发出 `Finish`：Chat 在用量块、Anthropic 在 `message_delta` 就能收尾，之后只剩
//!     纯终止符；代理或兼容端点省掉终止符就断开时，完整响应不能被尾部错误整体判失败。

use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::VecDeque;
use std::pin::Pin;
use zach_ai_core::{LanguageModelStream, ModelError, ModelWarning, StreamPart};

use super::sse::SseDecoder;

/// 协议解析器：把一帧 `data` 文本转换为零个或多个核心流事件。
pub(crate) trait SseParser: Send + 'static {
    /// 处理一帧 `data` 段。协议错误应以 [`StreamPart::Error`] 返回并置位 `failed`，
    /// 而不是 `Err`：解析错误不终止流，后续用量与元数据仍可收集。
    fn data(&mut self, data: &str) -> Vec<StreamPart>;

    /// 是否已收到协议层终止事件，之后不再读取网络。
    fn terminal(&self) -> bool;

    /// 是否已报告过错误事件，之后连接关闭不再视为传输故障。
    fn failed(&self) -> bool;

    /// 是否已发出 `Finish`（响应已语义完整），之后连接关闭不再视为传输故障。
    fn finished(&self) -> bool;
}

struct State<S, P> {
    source: Pin<Box<S>>,
    decoder: SseDecoder,
    parser: P,
    label: &'static str,
    queue: VecDeque<Result<StreamPart, ModelError>>,
    ended: bool,
}

/// 以 `parser` 驱动 `source`，`label` 用于错误信息中标识协议；
/// `warnings` 是请求构建阶段产生的降级警告，随首个 `StreamStart` 事件透出。
pub(crate) fn sse_stream<S, E, P>(
    label: &'static str,
    source: S,
    parser: P,
    warnings: Vec<ModelWarning>,
) -> LanguageModelStream
where
    S: Stream<Item = Result<Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
    P: SseParser,
{
    let state = State {
        source: Box::pin(source),
        decoder: SseDecoder::new(label),
        parser,
        label,
        queue: VecDeque::from([Ok(StreamPart::StreamStart { warnings })]),
        ended: false,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(part) = state.queue.pop_front() {
                return Some((part, state));
            }
            if state.ended {
                return None;
            }
            match state.source.next().await {
                Some(Ok(bytes)) => state.feed(&bytes),
                Some(Err(error)) => {
                    state.ended = true;
                    if !state.settled() {
                        state.queue.push_back(Err(ModelError::stream_error(
                            format!("读取 {} SSE 字节流失败", state.label),
                            error,
                        )));
                    }
                }
                None => {
                    state.ended = true;
                    if !state.settled() {
                        state.queue.push_back(Err(ModelError::StreamError {
                            message: format!("{} SSE 在终止事件到达前结束", state.label),
                            source: None,
                        }));
                    }
                }
            }
        }
    }))
}

impl<S, P: SseParser> State<S, P> {
    /// 本轮结果是否已由事件决定（失败或完成），之后连接如何结束只是诊断信息。
    fn settled(&self) -> bool {
        self.parser.failed() || self.parser.finished()
    }

    fn feed(&mut self, bytes: &[u8]) {
        match self.decoder.push(bytes) {
            Ok(frames) => {
                for frame in frames {
                    self.queue
                        .extend(self.parser.data(&frame).into_iter().map(Ok));
                    if self.parser.terminal() {
                        self.ended = true;
                        break;
                    }
                }
            }
            Err(error) => {
                self.queue.push_back(Err(error));
                self.ended = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! 驱动循环的终止语义：终止事件、错误事件与连接断开的组合。
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
        sse_stream("测试", source, parser(), vec![]).collect().await
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
        let parts: Vec<_> = sse_stream("测试", source, parser(), vec![]).collect().await;
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
        let mut stream = sse_stream("测试", source, parser(), vec![]);
        assert!(matches!(
            stream.next().await,
            Some(Ok(StreamPart::StreamStart { .. }))
        ));
        drop(stream);
        assert!(dropped.load(Ordering::SeqCst));
    }
}
