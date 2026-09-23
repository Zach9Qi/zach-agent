//! Agent 运行时。在 `zach-ai-core` 中间形态之上做会话与循环。

pub mod config;
pub mod context;
pub mod error;
pub mod event;
pub mod hooks;
pub mod host;
pub mod tool;

pub use config::{LoopConfig, QueueMode, RetryPolicy};
pub use context::{AgentContext, RequestState};
pub use error::AgentError;
pub use event::AgentEvent;
pub use hooks::{AgentHooks, NoopHooks, ToolCallDecision, ToolCallInfo, TurnDecision, TurnInfo};
pub use host::{ApprovalDecision, ApprovalRequest, LoopHost};
pub use tool::{
    typed_tool, AgentTool, SharedTool, ToolContext, ToolError, ToolExecutionMode, ToolOutcome,
    Typed, TypedTool,
};
