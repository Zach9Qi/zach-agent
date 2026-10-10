//! zach-ai 集成测试：单二进制入口，各子模块按被测主题划分
//!
//! 只通过 crate 公开 API 验证对外契约。

mod catalog;

#[cfg(feature = "openai-responses")]
mod responses;

#[cfg(feature = "openai-chat")]
mod chat;

// 传输层为三家共用，但状态码分类、分块 SSE、中途断开等真实 HTTP 行为只能借某个适配器观察；
// 这里以 Chat Completions 为载体，因此与 `openai-chat` 同开关。门禁跑默认 feature，三家全开，
// 这组用例始终执行；只开其他 feature 的构建会少掉它们，换载体需要另写一套请求与分块夹具。
#[cfg(feature = "openai-chat")]
mod http;
#[cfg(feature = "openai-chat")]
mod support;

#[cfg(feature = "anthropic")]
mod anthropic;
