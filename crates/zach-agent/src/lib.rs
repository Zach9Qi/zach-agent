//! Agent 运行时。在 `zach-ai-core` 中间形态之上做会话、工具执行与循环。

pub mod error;
pub mod tool;

// 门面精选重导出
pub use error::AgentError;
pub use tool::{
    AsyncDeferredLoader, AsyncFnExecutor, DeferredLoader, DeferredToolMeta, SyncFnExecutor,
    ToolExecutor, ToolRegistry,
};
