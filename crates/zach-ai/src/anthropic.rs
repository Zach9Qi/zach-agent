//! Anthropic Messages API 适配器。
//!
//! 该模块只负责 Messages API 的 HTTP 协议与 zach-ai-core 契约之间的转换，
//! Agent 编排层不感知 Anthropic 的内容块类型与 SSE 事件名称。
//!
//! 默认发送到 `{base_url}/v1/messages`，以无状态方式回放历史；支持文本、
//! 思考块（含签名回传与 `redacted_thinking`）、函数工具、图片、PDF 与内联文本文档，
//! 以及提示缓存标记与服务端工具块的透传。
//!
//! 使用 `anthropic` feature 启用；HTTPS 还需要选择 TLS feature。
//!
//! ```no_run
//! use futures::StreamExt;
//! use zach_ai::AnthropicMessagesModel;
//! use zach_ai_core::{CallOptions, LanguageModel, Message, StreamPart};
//!
//! # async fn run(api_key: String, model_id: String) -> Result<(), Box<dyn std::error::Error>> {
//! let model = AnthropicMessagesModel::new(api_key, model_id);
//! let options = CallOptions::new(vec![Message::user("你好")]);
//! let mut stream = model.do_stream(options).await?;
//! while let Some(part) = stream.next().await {
//!     if let StreamPart::TextDelta { delta, .. } = part? {
//!         print!("{delta}");
//!     }
//! }
//! # Ok(())
//! # }
//! ```

mod request;
mod response;
mod stream;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::Client;
use std::fmt;
use std::sync::Arc;
use zach_ai_core::{
    CallOptions, GenerateResult, LanguageModel, LanguageModelStream, ModelError, ModelProfile,
};

use crate::transport;
use request::{build_request, BuiltRequest};
use response::parse_response;
use stream::{messages_stream, MessagesStreamParser};

/// 请求头 `anthropic-version` 的固定值。
const API_VERSION: &str = "2023-06-01";

/// 厂商标识与错误信息中的协议名称。
const PROVIDER: &str = "anthropic";
const LABEL: &str = "Anthropic Messages";

/// Anthropic Messages API 的模型客户端。
#[derive(Clone)]
pub struct AnthropicMessagesModel {
    client: Client,
    api_key: Arc<str>,
    base_url: Arc<str>,
    model_id: Arc<str>,
    default_headers: HeaderMap,
}

impl fmt::Debug for AnthropicMessagesModel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AnthropicMessagesModel")
            .field("base_url", &self.base_url)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

impl AnthropicMessagesModel {
    /// 使用 Anthropic 官方端点创建模型。
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self::with_client(Client::new(), api_key, model_id)
    }

    /// 使用自定义 HTTP 客户端创建模型，便于复用连接池和测试配置。
    pub fn with_client(
        client: Client,
        api_key: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            client,
            api_key: Arc::from(api_key.into()),
            base_url: Arc::from("https://api.anthropic.com"),
            model_id: Arc::from(model_id.into()),
            default_headers: HeaderMap::new(),
        }
    }

    /// 设置根地址，例如 `https://api.anthropic.com`；适配器会自动追加 `/v1/messages`。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        self.base_url = Arc::from(base_url.trim_end_matches('/').to_owned());
        self
    }

    /// 添加每次请求都会携带的 HTTP 头（如 `anthropic-beta`）。
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.default_headers.insert(name, value);
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    fn headers(&self, options: &CallOptions, betas: &[String]) -> Result<HeaderMap, ModelError> {
        if self.api_key.trim().is_empty() {
            return Err(ModelError::InvalidRequest("API Key 不能为空".into()));
        }
        let mut headers = self.default_headers.clone();
        headers.insert(
            "x-api-key",
            transport::sensitive_header(&self.api_key, "API Key")?,
        );
        headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        transport::extend_headers(&mut headers, options.headers.as_ref())?;
        if !betas.is_empty() {
            // 请求派生的 beta 标记与调用方显式配置的标记合并为一个逗号分隔的请求头。
            let mut all: Vec<String> = headers
                .get("anthropic-beta")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.split(',').map(|s| s.trim().to_owned()).collect())
                .unwrap_or_default();
            for beta in betas {
                if !all.contains(beta) {
                    all.push(beta.clone());
                }
            }
            let value = HeaderValue::from_str(&all.join(","))
                .map_err(|_| ModelError::InvalidRequest("anthropic-beta 包含非法字符".into()))?;
            headers.insert("anthropic-beta", value);
        }
        Ok(headers)
    }

    fn request(
        &self,
        options: &CallOptions,
        stream: bool,
    ) -> Result<(reqwest::Request, BuiltRequest), ModelError> {
        let built = build_request(&self.model_id, options, stream)?;
        let request = self
            .client
            .post(self.endpoint())
            .headers(self.headers(options, &built.betas)?)
            .header(ACCEPT, transport::accept(stream))
            .json(&built.body)
            .build()
            .map_err(|err| ModelError::InvalidRequest(format!("构建 {LABEL} 请求失败: {err}")))?;
        Ok((request, built))
    }

    async fn send(
        &self,
        options: &CallOptions,
        stream: bool,
    ) -> Result<(reqwest::Response, BuiltRequest), ModelError> {
        let (request, built) = self.request(options, stream)?;
        let response = transport::execute(&self.client, PROVIDER, LABEL, request).await?;
        Ok((response, built))
    }
}

#[async_trait]
impl LanguageModel for AnthropicMessagesModel {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn profile(&self) -> Option<&ModelProfile> {
        crate::ModelCatalog::builtin().get(self.provider(), self.model_id())
    }

    fn is_url_supported(&self, media_type: &str, url: &str) -> bool {
        (media_type.starts_with("image/") || media_type == "application/pdf")
            && (url.starts_with("https://") || url.starts_with("http://"))
    }

    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError> {
        let (response, built) = self.send(&options, false).await?;
        let (value, headers) = transport::read_json(response, LABEL).await?;
        let mut result = parse_response(value)?;
        result.request_body = Some(built.body);
        result.response_headers = Some(headers);
        Ok(result)
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let (response, _) = self.send(&options, true).await?;
        transport::require_event_stream(&response, PROVIDER)?;
        let parser = MessagesStreamParser::new(options.include_raw_chunks);
        Ok(messages_stream(response.bytes_stream(), parser))
    }
}
