//! `zach-ai-core`: 大语言模型（LLM）统一中间形态抽象层。
//!
//! 提供模型中立的消息模型、提示词系统、工具定义、流式事件生命周期与通用 Trait 契约。
//!
//! JSON 约定：枚举判别值与字段名统一为 `snake_case`。同一个概念只保留一种写法，
//! 例如工具调用在 [`StreamPart`]、[`OutputContent`]、[`AssistantPart`] 上都是 `tool_call`，
//! 结束原因 [`UnifiedFinishReason::ToolCalls`] 是 `tool_calls`。

pub mod call_options;
pub mod error;
pub mod file;
pub mod model;
pub mod model_profile;
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
pub use model_profile::{
    CostBreakdown, Modalities, Modality, ModelLimits, ModelPricing, ModelProfile, ModelStatus,
    PricingRates, PricingTier, ReasoningProfile,
};
pub use options::{ModelWarning, ProviderMetadata, ProviderOptions};
pub use prompt::{AssistantPart, Message, Prompt, ToolPart, UserPart};
pub use response::{
    FinishReason, GenerateResult, InputTokenUsage, OutputContent, OutputTokenUsage,
    ResponseMetadata, SourceContent, UnifiedFinishReason, Usage,
};
pub use stream::{StreamAccumulator, StreamPart, StreamPartError};
pub use tool::{
    FunctionTool, ProviderTool, ToolChoice, ToolDefinition, ToolResultContentBlock,
    ToolResultOutput,
};
