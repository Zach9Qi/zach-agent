//! zach-ai 集成测试：单二进制入口，各子模块按被测主题划分
//!
//! 只通过 crate 公开 API 验证对外契约。

mod catalog;

#[cfg(feature = "openai-responses")]
mod responses;

#[cfg(feature = "openai-chat")]
mod chat;

#[cfg(feature = "openai-chat")]
mod http;
#[cfg(feature = "openai-chat")]
mod support;

#[cfg(feature = "anthropic")]
mod anthropic;
