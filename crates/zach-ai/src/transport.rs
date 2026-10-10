//! 三家适配器共用的 HTTP / SSE 传输层。
//!
//! 这里只处理字节与 HTTP 状态：请求头拼装、非 2xx 的错误分类、SSE 帧解码，
//! 以及"字节流 → 帧 → 事件"的驱动循环。协议语义（事件如何映射为 [`zach_ai_core::StreamPart`]）
//! 留在各适配器的解析器内，通过 [`SseParser`] 接入驱动。

mod http;
mod sse;
mod stream;

pub(crate) use http::{
    accept, default_client, execute, extend_headers, read_json, require_event_stream,
    sensitive_header,
};
pub(crate) use stream::{sse_stream, SseParser};
