//! Agent 运行时统一错误定义

use thiserror::Error;
use zach_ai_core::ModelError;

/// Agent 运行过程中的错误类型
#[derive(Debug, Error)]
pub enum AgentError {
    /// 底层模型调用失败（重试用尽或不可重试）
    #[error("底层模型或解析器错误: {0}")]
    Model(#[from] ModelError),

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

    /// 内部状态或通用错误
    #[error("Agent 内部错误: {0}")]
    Internal(String),
}
