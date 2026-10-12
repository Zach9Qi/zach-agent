//! 会话运行：在后台任务中准备上下文、启动 Agent 并转发事件
//!
//! 运行开始前以会话投影覆盖 Agent 的对话记录：会话是唯一事实来源，
//! Agent 内部记录在运行中途与投影不一致也不影响下一次运行。

use super::error::SessionError;
use super::handle::Session;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::task::{Context, Poll};
use tokio::sync::{mpsc, oneshot};
use zach_agent::{AgentEvent, AgentRun, RunOutput};
use zach_ai_core::Message;

/// 事件通道容量
const EVENT_BUFFER: usize = 256;

/// 会话运行中产生的事件
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SessionEvent {
    /// Agent 的运行时事件，原样转发
    Agent(AgentEvent),
}

/// 一次会话运行：`Stream<Item = SessionEvent>`，消费完事件后用 [`Self::outcome`] 取得结果
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

/// 在后台任务中驱动一次运行；调用方已占用会话的运行标记
pub(super) fn spawn(session: Session, start: Start) -> SessionRun {
    let (sender, events) = mpsc::channel(EVENT_BUFFER);
    let (done, outcome) = oneshot::channel();
    tokio::spawn(async move {
        let mut result = drive(&session, start, &sender).await;
        if let Err(error) = session.shared.sync().await {
            result = result.and(Err(error));
        }
        session.shared.running.store(false, Ordering::SeqCst);
        let _ = done.send(result);
    });
    SessionRun { events, outcome }
}

async fn drive(
    session: &Session,
    start: Start,
    sender: &mpsc::Sender<SessionEvent>,
) -> Result<RunOutput, SessionError> {
    session.shared.ensure_healthy()?;
    let context = session.context().await?;
    session.agent.set_messages(context);
    session.record_config().await?;
    let run = match start {
        Start::Prompt(messages) => session.agent.prompt(messages)?,
        Start::Continue => session.agent.continue_run()?,
    };
    let output = forward(run, sender).await?;
    // 写入失败时钩子会提前结束运行，这里以写入错误报告
    session.shared.ensure_healthy()?;
    Ok(output)
}

/// 转发事件直到运行结束；使用方不再消费时立即放弃事件，避免审批请求悬挂
async fn forward(
    mut run: AgentRun,
    sender: &mpsc::Sender<SessionEvent>,
) -> Result<RunOutput, SessionError> {
    while let Some(event) = run.next().await {
        if sender.send(SessionEvent::Agent(event)).await.is_err() {
            break;
        }
    }
    Ok(run.outcome().await?)
}
