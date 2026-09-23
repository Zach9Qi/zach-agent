//! 工具执行错误

use std::fmt::Display;
use thiserror::Error;

/// 工具入参校验或执行失败
///
/// 循环会把错误转成 [`zach_ai_core::ToolResultOutput::ErrorText`] 回传给模型，不会终止运行。
#[derive(Debug, Error)]
pub enum ToolError {
    /// 入参不符合工具约定（缺字段、类型错误等）
    #[error("工具入参无效: {0}")]
    InvalidInput(String),

    /// 执行过程失败
    #[error("{0}")]
    Failed(String),
}

impl ToolError {
    /// 构造入参无效错误
    pub fn invalid_input(message: impl Display) -> Self {
        Self::InvalidInput(message.to_string())
    }

    /// 构造执行失败错误
    pub fn failed(message: impl Display) -> Self {
        Self::Failed(message.to_string())
    }
}
