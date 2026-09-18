//! 工具契约与执行结果模块入口

pub mod definition;
pub mod result;

pub use definition::{FunctionTool, ProviderTool, ToolChoice, ToolDefinition};
pub use result::{ToolResultContentBlock, ToolResultOutput};
