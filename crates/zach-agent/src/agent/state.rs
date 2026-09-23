//! Agent 共享状态：对话记录、运行配置、排队消息与审批等待表

use crate::config::{LoopConfig, QueueMode, RetryPolicy};
use crate::context::AgentContext;
use crate::event::AgentEvent;
use crate::hooks::AgentHooks;
use crate::host::ApprovalDecision;
use crate::tool::{SharedTool, ToolExecutionMode};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use zach_ai_core::{CallOptions, LanguageModel, Message, ProviderTool};

/// 排队消息
#[derive(Debug, Default)]
pub(super) struct PendingQueue {
    messages: VecDeque<Message>,
    pub(super) mode: QueueMode,
}

impl PendingQueue {
    pub(super) fn new(mode: QueueMode) -> Self {
        Self {
            messages: VecDeque::new(),
            mode,
        }
    }

    pub(super) fn push(&mut self, message: Message) {
        self.messages.push_back(message);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// 按模式预览下一个注入点会取出的消息
    pub(super) fn peek(&self) -> Vec<Message> {
        match self.mode {
            QueueMode::All => self.messages.iter().cloned().collect(),
            QueueMode::OneAtATime => self.messages.front().cloned().into_iter().collect(),
        }
    }

    /// 按模式取出消息
    pub(super) fn drain(&mut self) -> Vec<Message> {
        let count = match self.mode {
            QueueMode::All => self.messages.len(),
            QueueMode::OneAtATime => self.messages.len().min(1),
        };
        self.messages.drain(..count).collect()
    }

    pub(super) fn clear(&mut self) {
        self.messages.clear();
    }
}

/// 受互斥锁保护的全部可变状态；锁从不跨越 `await`
pub(super) struct State {
    pub(super) model: Arc<dyn LanguageModel>,
    pub(super) options: CallOptions,
    pub(super) hooks: Arc<dyn AgentHooks>,
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
    /// `true` 表示空闲
    pub(super) idle: watch::Sender<bool>,
}

impl Inner {
    pub(super) fn new(state: State) -> Self {
        Self {
            state: Mutex::new(state),
            idle: watch::Sender::new(true),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 运行结束（含异常退出）后清理运行态并标记空闲
    pub(super) fn finish_run(&self) {
        {
            let mut state = self.lock();
            state.active = None;
            state.pending_tool_calls.clear();
            state.approvals.clear();
        }
        self.idle.send_replace(true);
    }
}

/// 任务退出时（包括 panic）保证运行态被清理
pub(super) struct RunGuard(pub(super) Arc<Inner>);

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.finish_run();
    }
}
