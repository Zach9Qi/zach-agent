//! 工具抽象：循环可调度的本地工具契约与类型化辅助

mod context;
mod error;
mod typed;
mod types;

pub use context::ToolContext;
pub use error::ToolError;
pub use typed::{typed_tool, Typed, TypedTool};
pub use types::{AgentTool, SharedTool, ToolExecutionMode, ToolOutcome};
