//! 有状态 Agent：持有对话记录与配置，驱动低层循环并提供运行中的控制接口
//!
//! `Agent` 是可廉价克隆的句柄，可在其他任务中调用 `steer`、`abort`、`respond_approval`。
//! 发起运行需要处于 tokio 运行时中。

mod accessors;
mod builder;
mod host;
mod run;
mod state;

pub use builder::AgentBuilder;
pub use run::AgentRun;

use crate::agent_loop::{continue_agent_loop, run_agent_loop};
use crate::error::AgentError;
use crate::host::ApprovalDecision;
use host::RunHost;
use state::{Inner, RunGuard};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zach_ai_core::{LanguageModel, Message};

/// 事件通道容量
const EVENT_BUFFER: usize = 256;

/// 有状态 Agent 句柄
#[derive(Clone)]
pub struct Agent {
    inner: Arc<Inner>,
}

impl Agent {
    /// 以模型开始构造
    pub fn builder(model: Arc<dyn LanguageModel>) -> AgentBuilder {
        AgentBuilder::new(model)
    }

    /// 以默认配置构造
    pub fn new(model: Arc<dyn LanguageModel>) -> Self {
        Self::builder(model).build()
    }

    /// 以一条用户文本开始运行
    pub fn prompt_text(&self, text: impl Into<String>) -> Result<AgentRun, AgentError> {
        self.prompt(vec![Message::user(text)])
    }

    /// 以一批消息开始运行。已有运行时返回 [`AgentError::Busy`]，请改用 `steer` / `follow_up`。
    pub fn prompt(&self, messages: Vec<Message>) -> Result<AgentRun, AgentError> {
        self.start(Some(messages), false)
    }

    /// 从当前对话记录继续
    ///
    /// 最后一条是助手消息时，依次尝试注入排队的插队消息、追加消息；都没有则报错。
    pub fn continue_run(&self) -> Result<AgentRun, AgentError> {
        let queued = {
            let mut state = self.inner.lock();
            if state.active.is_some() {
                return Err(AgentError::Busy);
            }
            match state.messages.last() {
                None => return Err(AgentError::NoMessages),
                Some(Message::Assistant { .. }) => {
                    let steering = state.steering.drain();
                    if !steering.is_empty() {
                        Some((steering, true))
                    } else {
                        let follow_up = state.follow_up.drain();
                        if follow_up.is_empty() {
                            return Err(AgentError::CannotContinueFromAssistant);
                        }
                        Some((follow_up, false))
                    }
                }
                Some(_) => None,
            }
        };
        match queued {
            Some((messages, skip_initial_steering)) => {
                self.start(Some(messages), skip_initial_steering)
            }
            None => self.start(None, false),
        }
    }

    /// 排队一条插队消息：在当前轮次的工具执行完后、下一次请求前注入
    pub fn steer(&self, message: Message) {
        self.inner.lock().steering.push(message);
    }

    /// 排队一条追加消息：在运行本应结束时注入并继续
    pub fn follow_up(&self, message: Message) {
        self.inner.lock().follow_up.push(message);
    }

    /// 中止当前运行（若有）
    pub fn abort(&self) {
        if let Some(cancel) = &self.inner.lock().active {
            cancel.cancel();
        }
    }

    /// 等待当前运行结束
    pub async fn wait_for_idle(&self) {
        let mut idle = self.inner.idle.subscribe();
        let _ = idle.wait_for(|idle| *idle).await;
    }

    /// 答复一个待审批请求
    pub fn respond_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AgentError> {
        let sender = self.inner.lock().approvals.remove(approval_id);
        let sender = sender.ok_or_else(|| AgentError::ApprovalNotFound(approval_id.to_string()))?;
        sender
            .send(decision)
            .map_err(|_| AgentError::ApprovalNotFound(approval_id.to_string()))
    }

    /// 清空对话记录、排队消息与错误状态。运行中调用返回 [`AgentError::Busy`]。
    pub fn reset(&self) -> Result<(), AgentError> {
        let mut state = self.inner.lock();
        if state.active.is_some() {
            return Err(AgentError::Busy);
        }
        state.messages.clear();
        state.steering.clear();
        state.follow_up.clear();
        state.pending_tool_calls.clear();
        state.last_error = None;
        Ok(())
    }

    fn start(
        &self,
        prompts: Option<Vec<Message>>,
        skip_initial_steering: bool,
    ) -> Result<AgentRun, AgentError> {
        let cancel = CancellationToken::new();
        let (context, config) = {
            let mut state = self.inner.lock();
            if state.active.is_some() {
                return Err(AgentError::Busy);
            }
            state.active = Some(cancel.clone());
            state.pending_tool_calls.clear();
            state.last_error = None;
            (state.context_snapshot(), state.loop_config())
        };
        self.inner.idle.send_replace(false);

        let (sender, receiver) = mpsc::channel(EVENT_BUFFER);
        let host = RunHost::new(self.inner.clone(), sender, skip_initial_steering);
        let guard = RunGuard(self.inner.clone());
        let token = cancel.clone();
        let handle = tokio::spawn(async move {
            let _guard = guard;
            match prompts {
                Some(prompts) => run_agent_loop(prompts, context, config, &host, token).await,
                None => continue_agent_loop(context, config, &host, token).await,
            }
        });
        Ok(AgentRun::new(receiver, handle, cancel))
    }
}
