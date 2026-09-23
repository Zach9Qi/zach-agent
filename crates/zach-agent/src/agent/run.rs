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
/// 作为 `Stream<Item = AgentEvent>` 逐个产出事件；事件通道有界，不消费会阻塞运行。
/// 丢弃句柄不会中止运行，只是不再接收事件；要中止请调用 [`Self::abort`]。
pub struct AgentRun {
    events: mpsc::Receiver<AgentEvent>,
    handle: JoinHandle<Result<RunOutput, AgentError>>,
    cancel: CancellationToken,
}

impl AgentRun {
    pub(super) fn new(
        events: mpsc::Receiver<AgentEvent>,
        handle: JoinHandle<Result<RunOutput, AgentError>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            events,
            handle,
            cancel,
        }
    }

    /// 中止本次运行
    pub fn abort(&self) {
        self.cancel.cancel();
    }

    /// 本次运行的取消令牌
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// 放弃剩余事件，等待运行结束并返回结果
    pub async fn outcome(self) -> Result<RunOutput, AgentError> {
        let Self { events, handle, .. } = self;
        drop(events);
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
        self.get_mut().events.poll_recv(cx)
    }
}
