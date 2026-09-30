//! 一次运行的句柄：事件流、中止与最终结果

use crate::agent_loop::RunOutput;
use crate::error::AgentError;
use crate::event::AgentEvent;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 进行中的运行
///
/// 作为 `Stream<Item = AgentEvent>` 逐个产出事件；普通事件通道有界，正常运行时不消费会阻塞运行。
/// 中止会打断事件发送等待；中止后普通事件仅尽力投递，队列满时可能丢弃。
/// `RunAbort` / `RunError` / `RunFinish` 独立保存，在已排队的普通事件之后产出。
/// 丢弃句柄不会中止运行，只是不再接收事件；要中止请调用 [`Self::abort`]。
///
/// 丢弃句柄后运行中转人工的审批请求会被自动拒绝（无人可答复）；
/// 无人值守场景请通过 [`crate::AgentBuilder::approval`] 配置审批处理器。
pub struct AgentRun {
    events: mpsc::Receiver<AgentEvent>,
    terminal: mpsc::Receiver<AgentEvent>,
    handle: JoinHandle<Result<RunOutput, AgentError>>,
    cancel: CancellationToken,
}

impl AgentRun {
    pub(super) fn new(
        events: mpsc::Receiver<AgentEvent>,
        terminal: mpsc::Receiver<AgentEvent>,
        handle: JoinHandle<Result<RunOutput, AgentError>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            events,
            terminal,
            handle,
            cancel,
        }
    }

    /// 中止本次运行，不要求继续消费事件；运行仍会完成工具结果补齐等收尾。
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// 本次运行的取消令牌
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// 放弃剩余事件，等待运行结束并返回结果
    ///
    /// 之后产生的交互式审批请求将无人答复而被自动拒绝，见 [`crate::ApprovalHandler`]。
    pub async fn outcome(self) -> Result<RunOutput, AgentError> {
        let Self {
            events,
            terminal,
            handle,
            ..
        } = self;
        drop(events);
        drop(terminal);
        join(handle).await
    }

    /// 收集全部事件并返回结果
    pub async fn collect(mut self) -> (Vec<AgentEvent>, Result<RunOutput, AgentError>) {
        let mut events = Vec::new();
        while let Some(event) = self.next().await {
            events.push(event);
        }
        (events, join(self.handle).await)
    }
}

async fn join(handle: JoinHandle<Result<RunOutput, AgentError>>) -> Result<RunOutput, AgentError> {
    handle
        .await
        .map_err(|error| AgentError::Internal(format!("运行任务异常退出: {error}")))?
}

impl Stream for AgentRun {
    type Item = AgentEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.events.poll_recv(cx) {
            // 普通通道关闭代表宿主已退出，确保普通事件排空且收尾已完成后才产出终止事件。
            Poll::Ready(None) => this.terminal.poll_recv(cx),
            event => event,
        }
    }
}
