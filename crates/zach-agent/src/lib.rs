//! Agent 运行时。在 `zach-ai-core` 中间形态之上做会话与循环。

pub mod agent;
pub mod agent_loop;
pub mod config;
pub mod context;
pub mod error;
pub mod event;
pub mod hooks;
pub mod host;
pub mod tool;
mod utils;

pub use agent::{Agent, AgentBuilder, AgentRun};
pub use agent_loop::{continue_agent_loop, run_agent_loop, RunOutput};
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
