//! 循环上下文：对话记录、系统提示词与可用工具

use crate::tool::SharedTool;
use std::fmt;
use std::sync::Arc;
use zach_ai_core::{CallOptions, LanguageModel, Message, ProviderTool, ToolDefinition};

/// 一次运行可见的上下文快照
#[derive(Clone, Default)]
pub struct AgentContext {
    /// 系统提示词，每次请求时置于消息最前
    pub system_prompt: Option<String>,
    /// 对话记录（不含系统提示词）
    pub messages: Vec<Message>,
    /// 本地可执行工具
    pub tools: Vec<SharedTool>,
    /// 厂商原生工具（只声明，由厂商执行）
    pub provider_tools: Vec<ProviderTool>,
}

impl AgentContext {
    /// 按名称查找本地工具
    pub fn find_tool(&self, name: &str) -> Option<&SharedTool> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    /// 本次请求的工具声明；没有任何工具时返回 `None`
    pub fn tool_definitions(&self) -> Option<Vec<ToolDefinition>> {
        let definitions: Vec<ToolDefinition> = self
            .tools
            .iter()
            .map(|tool| ToolDefinition::Function(tool.definition().clone()))
            .chain(
                self.provider_tools
                    .iter()
                    .cloned()
                    .map(ToolDefinition::Provider),
            )
            .collect();
        (!definitions.is_empty()).then_some(definitions)
    }
}

impl fmt::Debug for AgentContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tools: Vec<&str> = self.tools.iter().map(|tool| tool.name()).collect();
        f.debug_struct("AgentContext")
            .field("system_prompt", &self.system_prompt)
            .field("messages", &self.messages)
            .field("tools", &tools)
            .field("provider_tools", &self.provider_tools)
            .finish()
    }
}

/// 发起模型请求前的可变运行态
///
/// 钩子对它的修改会作用于本次及之后的请求。
#[derive(Clone)]
pub struct RequestState {
    /// 本次请求使用的模型
    pub model: Arc<dyn LanguageModel>,
    /// 调用参数模板；`prompt` 与 `tools` 由循环按上下文填充
    pub options: CallOptions,
    /// 上下文
    pub context: AgentContext,
}

impl fmt::Debug for RequestState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestState")
            .field("provider", &self.model.provider())
            .field("model_id", &self.model.model_id())
            .field("options", &self.options)
            .field("context", &self.context)
            .finish()
    }
}
