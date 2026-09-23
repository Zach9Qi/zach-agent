//! `Agent` 状态读写。修改在下一次运行时生效，不影响进行中的运行。

use super::Agent;
use crate::config::{QueueMode, RetryPolicy};
use crate::hooks::AgentHooks;
use crate::tool::{SharedTool, ToolExecutionMode};
use std::sync::Arc;
use zach_ai_core::{CallOptions, LanguageModel, Message, ProviderTool, ReasoningEffort};

impl Agent {
    /// 对话记录快照（运行中会随消息写入实时增长）
    pub fn messages(&self) -> Vec<Message> {
        self.inner.lock().messages.clone()
    }

    /// 替换对话记录
    pub fn set_messages(&self, messages: Vec<Message>) {
        self.inner.lock().messages = messages;
    }

    /// 系统提示词
    pub fn system_prompt(&self) -> Option<String> {
        self.inner.lock().system_prompt.clone()
    }

    /// 设置系统提示词
    pub fn set_system_prompt(&self, prompt: Option<String>) {
        self.inner.lock().system_prompt = prompt;
    }

    /// 当前模型
    pub fn model(&self) -> Arc<dyn LanguageModel> {
        self.inner.lock().model.clone()
    }

    /// 替换模型
    pub fn set_model(&self, model: Arc<dyn LanguageModel>) {
        self.inner.lock().model = model;
    }

    /// 本地工具
    pub fn tools(&self) -> Vec<SharedTool> {
        self.inner.lock().tools.clone()
    }

    /// 替换本地工具
    pub fn set_tools(&self, tools: Vec<SharedTool>) {
        self.inner.lock().tools = tools;
    }

    /// 替换厂商原生工具
    pub fn set_provider_tools(&self, tools: Vec<ProviderTool>) {
        self.inner.lock().provider_tools = tools;
    }

    /// 调用参数模板
    pub fn options(&self) -> CallOptions {
        self.inner.lock().options.clone()
    }

    /// 替换调用参数模板
    pub fn set_options(&self, options: CallOptions) {
        self.inner.lock().options = options;
    }

    /// 设置推理强度
    pub fn set_reasoning(&self, reasoning: Option<ReasoningEffort>) {
        self.inner.lock().options.reasoning = reasoning;
    }

    /// 替换策略钩子
    pub fn set_hooks(&self, hooks: impl AgentHooks + 'static) {
        self.inner.lock().hooks = Arc::new(hooks);
    }

    /// 设置工具批次执行方式
    pub fn set_tool_execution(&self, mode: ToolExecutionMode) {
        self.inner.lock().tool_execution = mode;
    }

    /// 设置重试策略
    pub fn set_retry(&self, retry: RetryPolicy) {
        self.inner.lock().retry = retry;
    }

    /// 插队消息取用方式
    pub fn steering_mode(&self) -> QueueMode {
        self.inner.lock().steering.mode
    }

    /// 设置插队消息取用方式
    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.inner.lock().steering.mode = mode;
    }

    /// 追加消息取用方式
    pub fn follow_up_mode(&self) -> QueueMode {
        self.inner.lock().follow_up.mode
    }

    /// 设置追加消息取用方式
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.inner.lock().follow_up.mode = mode;
    }

    /// 是否仍有排队消息
    pub fn has_queued_messages(&self) -> bool {
        let state = self.inner.lock();
        !state.steering.is_empty() || !state.follow_up.is_empty()
    }

    /// 预览下一个注入点会取出的消息（插队优先），不消费
    pub fn peek_queued_messages(&self) -> Vec<Message> {
        let state = self.inner.lock();
        let steering = state.steering.peek();
        if steering.is_empty() {
            state.follow_up.peek()
        } else {
            steering
        }
    }

    /// 清空插队消息
    pub fn clear_steering_queue(&self) {
        self.inner.lock().steering.clear();
    }

    /// 清空追加消息
    pub fn clear_follow_up_queue(&self) {
        self.inner.lock().follow_up.clear();
    }

    /// 清空全部排队消息
    pub fn clear_queues(&self) {
        let mut state = self.inner.lock();
        state.steering.clear();
        state.follow_up.clear();
    }

    /// 是否有运行在进行中
    pub fn is_running(&self) -> bool {
        self.inner.lock().active.is_some()
    }

    /// 已下发入参、尚未得出结果的工具调用 ID
    pub fn pending_tool_calls(&self) -> Vec<String> {
        self.inner.lock().pending_tool_calls.clone()
    }

    /// 等待答复的审批请求 ID
    pub fn pending_approvals(&self) -> Vec<String> {
        self.inner.lock().approvals.keys().cloned().collect()
    }

    /// 最近一次失败运行的错误信息
    pub fn last_error(&self) -> Option<String> {
        self.inner.lock().last_error.clone()
    }
}
