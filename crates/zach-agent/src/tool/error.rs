//! 工具执行错误

use std::fmt;
use zach_ai_core::error::report;

/// 工具入参校验或执行失败
///
/// 唯一的消费者是循环：它把错误渲染成文本，作为 [`zach_ai_core::ToolResultOutput::ErrorText`]
/// 回传给模型，不会终止运行。
///
/// 刻意不实现 [`std::error::Error`]（与 anyhow 同样的取舍），从而能为任意错误类型提供 `From`：
/// 工具实现里可以直接用 `?`，底层错误的 `source` 链会完整写进回传文本，模型看到的是根因而不只是
/// 最外层的一句话。经由 `?` 转换的错误一律归为 [`ToolErrorKind::Failed`]；入参问题请显式使用
/// [`Self::invalid_input`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
}

/// 工具错误类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// 入参不符合工具约定（缺字段、类型错误等）
    InvalidInput,
    /// 执行过程失败
    Failed,
}

impl ToolError {
    /// 构造入参无效错误
    pub fn invalid_input(message: impl fmt::Display) -> Self {
        Self {
            kind: ToolErrorKind::InvalidInput,
            message: message.to_string(),
        }
    }

    /// 构造执行失败错误
    pub fn failed(message: impl fmt::Display) -> Self {
        Self {
            kind: ToolErrorKind::Failed,
            message: message.to_string(),
        }
    }

    /// 错误类别
    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    /// 错误描述，不含类别前缀
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ToolErrorKind::InvalidInput => write!(f, "工具入参无效: {}", self.message),
            ToolErrorKind::Failed => f.write_str(&self.message),
        }
    }
}

impl<E: std::error::Error> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self::failed(report(&error))
    }
}
