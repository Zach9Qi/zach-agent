//! 工具契约与执行结果模块入口

pub mod definition;
pub mod input;
pub mod result;

pub use definition::{FunctionTool, ProviderTool, ToolChoice, ToolDefinition};
pub use input::{parse_tool_input, tool_input_for_display, tool_input_for_replay, ToolInputError};
pub use result::{ToolResultContentBlock, ToolResultOutput};
