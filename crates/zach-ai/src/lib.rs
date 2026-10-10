//! 厂商适配层。把 OpenAI / Anthropic 等协议映射为 `zach-ai-core` 中间形态。

pub mod catalog;

pub use catalog::ModelCatalog;

#[cfg(any(
    feature = "openai-responses",
    feature = "openai-chat",
    feature = "anthropic"
))]
mod transport;

#[cfg(feature = "openai-responses")]
pub mod responses;
#[cfg(feature = "openai-responses")]
pub use responses::OpenAiResponsesModel;

#[cfg(feature = "openai-chat")]
pub mod chat;
#[cfg(feature = "openai-chat")]
pub use chat::{OpenAiChatCompletionModel, OpenAiChatCompletionsModel, OpenAiChatModel};

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "anthropic")]
pub use anthropic::AnthropicMessagesModel;
