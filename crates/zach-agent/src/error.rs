//! Agent 运行时错误定义
//!
//! - [`AgentError`]：同步调用错误，前置条件不满足，运行没有开始；
//! - [`RunError`]：运行失败，携带失败前已完成的部分。
//!
//! 两者分开，函数签名本身就说明了可能的失败：`Agent::prompt` 只会因前置条件失败，
//! 模型错误只会在运行结束时以 [`RunError`] 出现。

use crate::agent_loop::RunOutput;
use std::fmt;
use thiserror::Error;
use zach_ai_core::{ErrorInfo, ErrorKind, ModelError};

/// 同步调用错误：前置条件不满足，运行没有开始
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AgentError {
    /// 已有运行在进行中，不能再发起新的运行或重置
    #[error("Agent 正在运行中，请使用 steer/follow_up 排队消息，或等待当前运行结束")]
    Busy,

    /// 对话记录为空，没有可继续的消息
    #[error("没有可继续的消息")]
    NoMessages,

    /// 对话记录最后一条是助手消息，且没有排队中的消息可以注入
    #[error("最后一条消息是助手消息，无法继续")]
    CannotContinueFromAssistant,

    /// 找不到对应的待审批请求（已答复、已超时或 ID 错误）
    #[error("找不到待审批请求: {0}")]
    ApprovalNotFound(String),
}

/// 运行失败
///
/// 对内层错误是透明的：`Display` 就是失败原因本身的摘要，`source` 接上它的原因链，
/// 因此直接打印能看到原因，[`zach_ai_core::error::report`] 也不会重复。
#[derive(Debug)]
#[non_exhaustive]
pub enum RunError {
    /// 对话记录不满足继续条件，运行没有开始（仅 [`crate::continue_agent_loop`] 产生）
    NotStarted(AgentError),

    /// 模型调用失败（重试用尽或不可重试）
    Model {
        /// 最后一次尝试的错误；类别与上下文见 [`ModelError::kind`] 等访问器
        error: ModelError,
        /// 失败前已完成的部分：已写入对话记录的消息、已成功轮次的用量
        partial: Box<RunOutput>,
    },

    /// 运行任务异常退出（钩子、宿主等处 panic，或任务被运行时取消）
    Internal(String),
}

impl RunError {
    /// 错误类别
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::NotStarted(_) => ErrorKind::InvalidRequest,
            Self::Model { error, .. } => error.kind(),
            Self::Internal(_) => ErrorKind::Internal,
        }
    }

    /// 可序列化的错误信息，与 [`crate::AgentEvent::RunError`] 携带的一致
    pub fn info(&self) -> ErrorInfo {
        match self {
            Self::NotStarted(error) => ErrorInfo::new(self.kind(), error.to_string()),
            Self::Model { error, .. } => ErrorInfo::from(error),
            Self::Internal(message) => ErrorInfo::new(self.kind(), message.clone()),
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotStarted(error) => error.fmt(f),
            Self::Model { error, .. } => error.fmt(f),
            Self::Internal(message) => write!(f, "运行任务异常退出: {message}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotStarted(error) => error.source(),
            Self::Model { error, .. } => error.source(),
            Self::Internal(_) => None,
        }
    }
}
