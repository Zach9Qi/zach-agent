//! Agent 运行时。在 `zach-ai-core` 中间形态之上做会话与循环。

pub mod error;
pub mod event;
pub mod tool;

pub use error::AgentError;
pub use event::AgentEvent;
pub use tool::{
    typed_tool, AgentTool, SharedTool, ToolContext, ToolError, ToolExecutionMode, ToolOutcome,
    Typed, TypedTool,
};
