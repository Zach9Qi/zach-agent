//! 工具契约与执行结果

use crate::tool::context::ToolContext;
use crate::tool::error::ToolError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use zach_ai_core::{FunctionTool, ToolResultOutput};

/// 同一条助手消息内多个工具调用的执行方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionMode {
    /// 依次准备，放行后并发执行；结果仍按模型给出的顺序写回
    #[default]
    Parallel,
    /// 逐个准备、执行、收尾
    Sequential,
}

/// 工具执行结果
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    /// 回传给模型的结果载荷
    pub output: ToolResultOutput,
    /// 请求在本批工具结束后停止运行。
    ///
    /// 仅当同一批次所有工具结果都为 `true` 时才会提前结束。
    pub terminate: bool,
}

impl ToolOutcome {
    /// 以任意结果载荷构造
    pub fn new(output: ToolResultOutput) -> Self {
        Self {
            output,
            terminate: false,
        }
    }

    /// 纯文本结果
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(ToolResultOutput::text(text))
    }

    /// JSON 结果
    pub fn json(value: Value) -> Self {
        Self::new(ToolResultOutput::json(value))
    }

    /// 文本错误结果
    pub fn error(text: impl Into<String>) -> Self {
        Self::new(ToolResultOutput::error_text(text))
    }

    /// 设置提前结束提示
    pub fn with_terminate(mut self, terminate: bool) -> Self {
        self.terminate = terminate;
        self
    }

    /// 结果是否为错误
    pub fn is_error(&self) -> bool {
        matches!(
            self.output,
            ToolResultOutput::ErrorText { .. } | ToolResultOutput::ErrorJson { .. }
        )
    }
}

impl From<ToolResultOutput> for ToolOutcome {
    fn from(output: ToolResultOutput) -> Self {
        Self::new(output)
    }
}

/// 循环可调度的本地工具
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// 发给模型的工具声明
    fn definition(&self) -> &FunctionTool;

    /// 工具名称，默认取自声明
    fn name(&self) -> &str {
        &self.definition().name
    }

    /// 界面展示用的友好标题
    fn title(&self) -> Option<&str> {
        None
    }

    /// 单工具执行方式。任一调用为 `Sequential` 时，整批改为串行。
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }

    /// 执行前对入参做兼容转换与校验，失败时不会执行工具
    fn prepare_input(&self, input: Value) -> Result<Value, ToolError> {
        Ok(input)
    }

    /// 本次调用是否需要人工审批
    fn needs_approval(&self, input: &Value) -> bool {
        let _ = input;
        false
    }

    /// 执行工具。失败请返回 `Err`，而不是把错误编码进成功结果。
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutcome, ToolError>;
}

/// 可跨任务共享的工具句柄
pub type SharedTool = Arc<dyn AgentTool>;
