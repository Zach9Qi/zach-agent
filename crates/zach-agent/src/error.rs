//! Agent 运行时统一错误定义

use thiserror::Error;
use zach_ai_core::ModelError;

/// Agent 运行与工具调度过程中的错误类型
#[derive(Debug, Error)]
pub enum AgentError {
    /// 目标工具未在注册表中找到
    #[error("工具未找到: {0}")]
    ToolNotFound(String),

    /// 延迟工具动态加载失败
    #[error("动态加载延迟工具 '{name}' 失败: {reason}")]
    DeferredToolLoadFailed { name: String, reason: String },

    /// 工具入参解析或校验失败
    #[error("工具 '{0}' 参数不合法: {1}")]
    InvalidToolInput(String, String),

    /// 工具执行过程中抛出异常
    #[error("工具 '{0}' 执行失败: {1}")]
    ToolExecutionFailed(String, String),

    /// 动态解析器解析异常
    #[error("底层模型或解析器错误: {0}")]
    Model(#[from] ModelError),

    /// 内部状态或通用错误
    #[error("Agent 内部错误: {0}")]
    Internal(String),
}
