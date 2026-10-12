//! 会话运行：在后台任务中准备上下文、启动 Agent、转发事件，并处理压缩与上下文超长重试
//!
//! 运行开始前以会话投影覆盖 Agent 的对话记录：会话是唯一事实来源，
//! Agent 内部记录在运行中途与投影不一致也不影响下一次运行。

use super::compaction::CompactionEvent;
use super::entry::CompactionReason;
use super::error::SessionError;
use super::handle::Session;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
use zach_agent::{AgentEvent, AgentRun, RunOutput};
use zach_ai_core::{ErrorKind, Message};

/// 事件通道容量
const EVENT_BUFFER: usize = 256;

/// 会话运行中产生的事件
// 绝大多数事件是 Agent 变体（流式增量），装箱会让每个事件多一次分配，只为缩小罕见的压缩事件
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// Agent 的运行时事件，原样转发
    Agent(AgentEvent),
    /// 上下文压缩的进展
    Compaction(CompactionEvent),
    /// 会话写入失败（每个会话只报告一次）：运行随即停止执行工具并结束，之后拒绝新的运行
    PersistFailed { reason: String },
}

/// 一次会话运行：`Stream<Item = SessionEvent>`，消费完事件后用 [`Self::outcome`] 取得结果
///
/// 模型报告上下文超长时，会话压缩后自动继续一次：这次失败的 `RunError` 事件不会送出，
/// 取而代之的是压缩事件与继续运行的事件；重试仍失败才送出 `RunError`。
pub struct SessionRun {
    events: mpsc::Receiver<SessionEvent>,
    outcome: oneshot::Receiver<Result<RunOutput, SessionError>>,
}

impl SessionRun {
    /// 放弃剩余事件，等待运行结束并返回结果
    ///
    /// 与 `AgentRun::outcome` 一致：之后产生的交互式审批请求无人答复而被自动拒绝。
    pub async fn outcome(self) -> Result<RunOutput, SessionError> {
        drop(self.events);
        self.outcome
            .await
            .unwrap_or_else(|_| Err(SessionError::Internal("运行任务意外结束".into())))
    }
}

impl Stream for SessionRun {
    type Item = SessionEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.poll_recv(cx)
    }
}

/// 运行的起点
pub(super) enum Start {
    Prompt(Vec<Message>),
    Continue,
}

/// 事件出口：Agent 事件与会话事件（压缩）汇入同一个有界通道
struct Outlet {
    sender: mpsc::Sender<SessionEvent>,
    side: mpsc::UnboundedReceiver<SessionEvent>,
    /// 使用方已不再消费
    closed: bool,
}

impl Outlet {
    async fn send(&mut self, event: SessionEvent) {
        if !self.closed && self.sender.send(event).await.is_err() {
            self.closed = true;
        }
    }

    /// 送出已积压的会话事件
    async fn drain_side(&mut self) {
        while let Ok(event) = self.side.try_recv() {
            self.send(event).await;
        }
    }
}

/// 在后台任务中驱动一次运行；调用方已占用会话的运行标记
pub(super) fn spawn(session: Session, start: Start) -> SessionRun {
    let (sender, events) = mpsc::channel(EVENT_BUFFER);
    let (side_sender, side) = mpsc::unbounded_channel();
    let (done, outcome) = oneshot::channel();
    session.shared.set_events(Some(side_sender));
    tokio::spawn(async move {
        let mut outlet = Outlet {
            sender,
            side,
            closed: false,
        };
        let mut result = drive(&session, start, &mut outlet).await;
        if let Err(error) = session.shared.sync().await {
            result = result.and(Err(error));
        }
        session.shared.set_events(None);
        outlet.drain_side().await;
        session.shared.running.store(false, Ordering::SeqCst);
        let _ = done.send(result);
    });
    SessionRun { events, outcome }
}

async fn drive(
    session: &Session,
    start: Start,
    outlet: &mut Outlet,
) -> Result<RunOutput, SessionError> {
    session.shared.ensure_healthy()?;
    let model = session.agent.model();
    if session.shared.over_threshold(model.as_ref()) {
        // 预防性压缩失败不阻止运行：失败已作为事件报告，超长时还有溢出重试兜底
        let _ = session
            .shared
            .compact(&model, CompactionReason::Threshold)
            .await;
        outlet.drain_side().await;
    }
    session.agent.set_messages(session.shared.context().await?);
    session.record_config().await?;
    let mut run = match start {
        Start::Prompt(messages) => session.agent.prompt(messages)?,
        Start::Continue => session.agent.continue_run()?,
    };

    let mut retried = false;
    loop {
        let (result, held) = forward(run, outlet, !retried).await;
        let overflowed = matches!(
            &result,
            Err(SessionError::Run(error)) if error.kind() == ErrorKind::ContextLengthExceeded
        );
        if overflowed && !retried && session.shared.options.compaction.auto {
            retried = true;
            let model = session.agent.model();
            let compacted = session
                .shared
                .compact(&model, CompactionReason::Overflow)
                .await;
            outlet.drain_side().await;
            if let Ok(Some(_)) = compacted {
                session.agent.set_messages(session.shared.context().await?);
                run = session.agent.continue_run()?;
                continue;
            }
        }
        if let Some(event) = held {
            outlet.send(SessionEvent::Agent(event)).await;
        }
        let output = result?;
        // 写入失败时钩子会提前结束运行，这里以写入错误报告
        session.shared.ensure_healthy()?;
        return Ok(output);
    }
}

/// 转发事件直到运行结束；`hold_overflow` 时暂扣上下文超长的 `RunError` 事件并一并返回
///
/// 使用方不再消费时立即放弃剩余事件（`AgentRun::outcome`），避免审批请求悬挂。
async fn forward(
    mut run: AgentRun,
    outlet: &mut Outlet,
    hold_overflow: bool,
) -> (Result<RunOutput, SessionError>, Option<AgentEvent>) {
    let mut held = None;
    while !outlet.closed {
        tokio::select! {
            biased;
            Some(event) = outlet.side.recv() => outlet.send(event).await,
            event = run.next() => match event {
                None => break,
                Some(event) if hold_overflow && is_overflow(&event) => held = Some(event),
                Some(event) => outlet.send(SessionEvent::Agent(event)).await,
            },
        }
    }
    outlet.drain_side().await;
    (run.outcome().await.map_err(Into::into), held)
}

fn is_overflow(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::RunError { error } if error.kind == ErrorKind::ContextLengthExceeded)
}
