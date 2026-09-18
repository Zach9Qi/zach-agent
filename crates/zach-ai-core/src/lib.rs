//! `zach-ai-core`: 大语言模型（LLM）统一中间形态抽象层。
//!
//! 提供模型中立的消息模型、提示词系统、工具定义、流式事件生命周期与通用 Trait 契约。

pub mod call_options;
pub mod error;
pub mod file;
pub mod model;
pub mod options;
pub mod prompt;
pub mod response;
pub mod stream;
pub mod tool;

// 门面精选重导出（Façade Re-exports）
pub use call_options::{CallOptions, ReasoningEffort, ResponseFormat};
pub use error::ModelError;
pub use file::FileData;
pub use model::{LanguageModel, LanguageModelStream};
pub use options::{ModelWarning, ProviderMetadata, ProviderOptions};
pub use prompt::{AssistantPart, Message, Prompt, ToolPart, UserPart};
pub use response::{
    FinishReason, GenerateResult, InputTokenUsage, OutputContent, OutputTokenUsage,
    ResponseMetadata, SourceContent, UnifiedFinishReason, Usage,
};
pub use stream::{StreamAccumulator, StreamPart};
pub use tool::{
    FunctionTool, ProviderTool, ToolChoice, ToolDefinition, ToolResultContentBlock,
    ToolResultOutput,
};
