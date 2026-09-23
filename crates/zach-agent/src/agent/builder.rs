//! `Agent` 构造器

use super::state::{Inner, PendingQueue, State};
use super::Agent;
use crate::config::{QueueMode, RetryPolicy};
use crate::hooks::{AgentHooks, NoopHooks};
use crate::tool::{SharedTool, ToolExecutionMode};
use std::collections::HashMap;
use std::sync::Arc;
use zach_ai_core::{CallOptions, LanguageModel, Message, ProviderTool, ReasoningEffort};

/// [`Agent`] 构造器
pub struct AgentBuilder {
    model: Arc<dyn LanguageModel>,
    options: CallOptions,
    hooks: Arc<dyn AgentHooks>,
    tool_execution: ToolExecutionMode,
    retry: RetryPolicy,
    system_prompt: Option<String>,
    messages: Vec<Message>,
    tools: Vec<SharedTool>,
    provider_tools: Vec<ProviderTool>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
}

impl AgentBuilder {
    pub(super) fn new(model: Arc<dyn LanguageModel>) -> Self {
        Self {
            model,
            options: CallOptions::default(),
            hooks: Arc::new(NoopHooks),
            tool_execution: ToolExecutionMode::default(),
            retry: RetryPolicy::default(),
            system_prompt: None,
            messages: Vec::new(),
            tools: Vec::new(),
            provider_tools: Vec::new(),
            steering_mode: QueueMode::default(),
            follow_up_mode: QueueMode::default(),
        }
    }

    /// 系统提示词
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// 追加一个本地工具
    pub fn tool(mut self, tool: SharedTool) -> Self {
        self.tools.push(tool);
        self
    }

    /// 替换全部本地工具
    pub fn tools(mut self, tools: Vec<SharedTool>) -> Self {
        self.tools = tools;
        self
    }

    /// 追加一个厂商原生工具
    pub fn provider_tool(mut self, tool: ProviderTool) -> Self {
        self.provider_tools.push(tool);
        self
    }

    /// 调用参数模板（`prompt` 与 `tools` 会被忽略）
    pub fn options(mut self, options: CallOptions) -> Self {
        self.options = options;
        self
    }

    /// 推理强度
    pub fn reasoning(mut self, reasoning: ReasoningEffort) -> Self {
        self.options.reasoning = Some(reasoning);
        self
    }

    /// 策略钩子
    pub fn hooks(mut self, hooks: impl AgentHooks + 'static) -> Self {
        self.hooks = Arc::new(hooks);
        self
    }

    /// 工具批次执行方式
    pub fn tool_execution(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution = mode;
        self
    }

    /// 模型请求重试策略
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// 插队消息取用方式
    pub fn steering_mode(mut self, mode: QueueMode) -> Self {
        self.steering_mode = mode;
        self
    }

    /// 追加消息取用方式
    pub fn follow_up_mode(mut self, mode: QueueMode) -> Self {
        self.follow_up_mode = mode;
        self
    }

    /// 初始对话记录
    pub fn messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// 构造 Agent
    pub fn build(self) -> Agent {
        let state = State {
            model: self.model,
            options: self.options,
            hooks: self.hooks,
            tool_execution: self.tool_execution,
            retry: self.retry,
            system_prompt: self.system_prompt,
            messages: self.messages,
            tools: self.tools,
            provider_tools: self.provider_tools,
            steering: PendingQueue::new(self.steering_mode),
            follow_up: PendingQueue::new(self.follow_up_mode),
            active: None,
            pending_tool_calls: Vec::new(),
            approvals: HashMap::new(),
            last_error: None,
        };
        Agent {
            inner: Arc::new(Inner::new(state)),
        }
    }
}
