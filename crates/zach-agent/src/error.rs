//! Agent 运行时统一错误定义

use thiserror::Error;
use zach_ai_core::ModelError;

/// Agent 运行过程中的错误类型
#[derive(Debug, Error)]
pub enum AgentError {
    /// 动态解析器解析异常
    #[error("底层模型或解析器错误: {0}")]
    Model(#[from] ModelError),

    /// 内部状态或通用错误
    #[error("Agent 内部错误: {0}")]
    Internal(String),
}
