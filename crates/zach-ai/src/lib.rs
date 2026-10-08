//! 厂商适配层。把 OpenAI / Anthropic 等协议映射为 `zach-ai-core` 中间形态。

pub mod catalog;

pub use catalog::ModelCatalog;

#[cfg(feature = "openai-responses")]
pub mod responses;
#[cfg(feature = "openai-responses")]
pub use responses::OpenAiResponsesModel;
