//! OpenAI Responses API 适配器。
//!
//! 该模块只负责 HTTP 协议与 zach-ai-core 契约之间的转换，Agent 编排层不感知
//! Responses API 的事件名称和请求字段。
//!
//! 默认发送到 /v1/responses，以无状态方式回放历史；支持文本、推理摘要、
//! 函数调用、图片和 PDF。暂不支持服务端内置工具与后台任务。
//!
//! 使用 openai 或 openai-responses feature 启用；HTTPS 还需要选择 TLS feature。
//!
//! ```no_run
//! use futures::StreamExt;
//! use zach_ai::OpenAiResponsesModel;
//! use zach_ai_core::{CallOptions, LanguageModel, Message, StreamPart};
//!
//! # async fn run(api_key: String, model_id: String) -> Result<(), Box<dyn std::error::Error>> {
//! let model = OpenAiResponsesModel::new(api_key, model_id);
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

mod error;
mod request;
mod response;
mod stream;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::fmt;
use std::sync::Arc;
use zach_ai_core::{
    CallOptions, GenerateResult, LanguageModel, LanguageModelStream, ModelError, ModelProfile,
};

use error::http_error;
use request::build_request;
use response::parse_response;
use stream::{responses_stream, ResponsesStreamParser};

/// OpenAI Responses API 的模型客户端。
#[derive(Clone)]
pub struct OpenAiResponsesModel {
    client: Client,
    api_key: Arc<str>,
    base_url: Arc<str>,
    model_id: Arc<str>,
    default_headers: HeaderMap,
}

impl fmt::Debug for OpenAiResponsesModel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpenAiResponsesModel")
            .field("base_url", &self.base_url)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

impl OpenAiResponsesModel {
    /// 使用 OpenAI 官方端点创建模型。
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
            base_url: Arc::from("https://api.openai.com/v1"),
            model_id: Arc::from(model_id.into()),
            default_headers: HeaderMap::new(),
        }
    }

    /// 设置 Responses API 的根地址，例如 https://api.openai.com/v1。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        self.base_url = Arc::from(base_url.trim_end_matches('/').to_owned());
        self
    }

    /// 添加每次请求都会携带的 HTTP 头。
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.default_headers.insert(name, value);
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/responses", self.base_url)
    }

    fn headers(&self, options: &CallOptions) -> Result<HeaderMap, ModelError> {
        if self.api_key.trim().is_empty() {
            return Err(ModelError::InvalidRequest("API Key 不能为空".into()));
        }
        let mut headers = self.default_headers.clone();
        let auth = format!("Bearer {}", self.api_key);
        let mut auth = HeaderValue::from_str(&auth)
            .map_err(|_| ModelError::InvalidRequest("API Key 包含非法 HTTP 字符".into()))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(extra) = &options.headers {
            for (name, value) in extra {
                let name = HeaderName::try_from(name.as_str()).map_err(|_| {
                    ModelError::InvalidRequest(format!("非法 HTTP 请求头名称: {name}"))
                })?;
                let mut value = HeaderValue::from_str(value).map_err(|_| {
                    ModelError::InvalidRequest(format!("非法 HTTP 请求头值: {name}"))
                })?;
                if name == AUTHORIZATION {
                    value.set_sensitive(true);
                }
                headers.insert(name, value);
            }
        }
        Ok(headers)
    }

    fn request(&self, options: &CallOptions, stream: bool) -> Result<reqwest::Request, ModelError> {
        let body = build_request(&self.model_id, options, stream)?;
        self.client
            .post(self.endpoint())
            .headers(self.headers(options)?)
            .header(
                reqwest::header::ACCEPT,
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .json(&body)
            .build()
            .map_err(|err| ModelError::InvalidRequest(format!("构建 Responses 请求失败: {err}")))
    }

    async fn send(
        &self,
        options: &CallOptions,
        stream: bool,
    ) -> Result<reqwest::Response, ModelError> {
        let request = self.request(options, stream)?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|err| ModelError::stream_error("请求 OpenAI Responses API 失败", err))?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(http_error(response).await)
        }
    }
}

#[async_trait]
impl LanguageModel for OpenAiResponsesModel {
    fn provider(&self) -> &str {
        "openai"
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
        let response = self.send(&options, false).await?;
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|err| ModelError::stream_error("读取 OpenAI Responses API 响应失败", err))?;
        let value: serde_json::Value = serde_json::from_slice(&body)?;
        let mut result = parse_response(value)?;
        result.request_body = Some(build_request(&self.model_id, &options, false)?);
        result.response_headers = Some(
            headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.to_string(), value.to_owned()))
                })
                .collect(),
        );
        Ok(result)
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let response = self.send(&options, true).await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case("text/event-stream")
        {
            return Err(ModelError::provider_error(
                "openai",
                format!("期望 text/event-stream，收到 {content_type}"),
                None,
            ));
        }
        let parser = ResponsesStreamParser::new(options.include_raw_chunks);
        Ok(responses_stream(response.bytes_stream(), parser))
    }
}
