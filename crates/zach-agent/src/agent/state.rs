//! Agent 共享状态：对话记录、运行配置、排队消息与审批等待表

use super::queue::PendingQueue;
use crate::approval::{ApprovalDecision, ApprovalHandler};
use crate::config::{LoopConfig, RetryPolicy};
use crate::context::AgentContext;
use crate::event::AgentEvent;
use crate::hooks::AgentHooks;
use crate::tool::{SharedTool, ToolExecutionMode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use zach_ai_core::{CallOptions, LanguageModel, Message, ProviderTool};

/// 受互斥锁保护的全部可变状态；锁从不跨越 `await`
pub(super) struct State {
    pub(super) model: Arc<dyn LanguageModel>,
    pub(super) options: CallOptions,
    pub(super) hooks: Arc<dyn AgentHooks>,
    pub(super) approval: Arc<dyn ApprovalHandler>,
    pub(super) tool_execution: ToolExecutionMode,
    pub(super) retry: RetryPolicy,
    pub(super) system_prompt: Option<String>,
    pub(super) messages: Vec<Message>,
    pub(super) tools: Vec<SharedTool>,
    pub(super) provider_tools: Vec<ProviderTool>,
    pub(super) steering: PendingQueue,
    pub(super) follow_up: PendingQueue,
    pub(super) active: Option<CancellationToken>,
    pub(super) pending_tool_calls: Vec<String>,
    pub(super) approvals: HashMap<String, oneshot::Sender<ApprovalDecision>>,
    pub(super) last_error: Option<String>,
}

impl State {
    pub(super) fn context_snapshot(&self) -> AgentContext {
        AgentContext {
            system_prompt: self.system_prompt.clone(),
            messages: self.messages.clone(),
            tools: self.tools.clone(),
            provider_tools: self.provider_tools.clone(),
        }
    }

    pub(super) fn loop_config(&self) -> LoopConfig {
        LoopConfig {
            model: self.model.clone(),
            options: self.options.clone(),
            hooks: self.hooks.clone(),
            tool_execution: self.tool_execution,
            retry: self.retry.clone(),
        }
    }

    /// 依据事件归约运行态：跟踪进行中的工具调用与最近一次错误
    pub(super) fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::ToolInputAvailable { tool_call_id, .. } => {
                if !self.pending_tool_calls.contains(tool_call_id) {
                    self.pending_tool_calls.push(tool_call_id.clone());
                }
            }
            AgentEvent::ToolInputError { tool_call_id, .. }
            | AgentEvent::ToolOutputError { tool_call_id, .. }
            | AgentEvent::ToolOutputDenied { tool_call_id, .. }
            | AgentEvent::ToolOutputAvailable {
                tool_call_id,
                preliminary: false,
                ..
            } => self.pending_tool_calls.retain(|id| id != tool_call_id),
            AgentEvent::RunError { error_text } => self.last_error = Some(error_text.clone()),
            _ => {}
        }
    }
}

/// `Agent` 各句柄共享的内部结构
pub(super) struct Inner {
    state: Mutex<State>,
    /// 运行收尾计数，仅作唤醒通知，不承载状态：每次运行结束后加一。
    ///
    /// 是否空闲以 `State::active` 为准；这里只负责叫醒 `wait_for_idle` 去重新检查，
    /// 因此可以在锁外发送，避免同步锁内嵌套 watch 的锁与唤醒 waker。
    pub(super) finished: watch::Sender<u64>,
}

impl Inner {
    pub(super) fn new(state: State) -> Self {
        Self {
            state: Mutex::new(state),
            finished: watch::Sender::new(0),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 运行结束（含异常退出）后清理运行态，并在锁外通知等待者重新检查
    pub(super) fn finish_run(&self) {
        {
            let mut state = self.lock();
            state.active = None;
            state.pending_tool_calls.clear();
            state.approvals.clear();
        }
        self.finished.send_modify(|generation| *generation += 1);
    }
}

/// 任务退出时（包括 panic）保证运行态被清理
pub(super) struct RunGuard(pub(super) Arc<Inner>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.finish_run();
    }
}
