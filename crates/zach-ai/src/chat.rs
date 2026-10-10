//! OpenAI Chat Completions API 适配器。
//!
//! 该模块把 Chat Completions 的消息、工具和 SSE 分块转换为
//! `zach-ai-core` 的统一模型契约。端点默认为 `/v1/chat/completions`。

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
use stream::{chat_stream, ChatStreamParser};

/// OpenAI Chat Completions 模型客户端。
#[derive(Clone)]
pub struct OpenAiChatCompletionsModel {
    client: Client,
    api_key: Arc<str>,
    base_url: Arc<str>,
    model_id: Arc<str>,
    default_headers: HeaderMap,
}

/// `OpenAiChatModel` 是较短的兼容别名。
pub type OpenAiChatModel = OpenAiChatCompletionsModel;

/// `OpenAiChatCompletionModel` 是单数形式的兼容别名。
pub type OpenAiChatCompletionModel = OpenAiChatCompletionsModel;

impl fmt::Debug for OpenAiChatCompletionsModel {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("OpenAiChatCompletionsModel")
            .field("base_url", &self.base_url)
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

impl OpenAiChatCompletionsModel {
    /// 使用 OpenAI 官方端点创建模型。
    pub fn new(api_key: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self::with_client(Client::new(), api_key, model_id)
    }

    /// 使用自定义客户端创建模型。
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

    /// 设置包含版本路径的根地址，例如 `https://api.openai.com/v1`。
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Arc::from(base_url.into().trim_end_matches('/').to_owned());
        self
    }

    /// 添加每次请求都会携带的 HTTP 头。
    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.default_headers.insert(name, value);
        self
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn headers(&self, options: &CallOptions) -> Result<HeaderMap, ModelError> {
        if self.api_key.trim().is_empty() {
            return Err(ModelError::InvalidRequest("API Key 不能为空".into()));
        }
        let mut headers = self.default_headers.clone();
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", self.api_key))
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
            .map_err(|error| {
                ModelError::InvalidRequest(format!("构建 Chat Completions 请求失败: {error}"))
            })
    }

    async fn send(
        &self,
        options: &CallOptions,
        stream: bool,
    ) -> Result<reqwest::Response, ModelError> {
        let response = self
            .client
            .execute(self.request(options, stream)?)
            .await
            .map_err(|error| {
                ModelError::stream_error("请求 OpenAI Chat Completions 失败", error)
            })?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(http_error(response).await)
        }
    }
}

#[async_trait]
impl LanguageModel for OpenAiChatCompletionsModel {
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
        // Chat Completions 的 `file` 内容块只接受 file_id 或内联数据，
        // PDF 等文件 URL 会在请求构建时被拒绝，这里必须保持一致。
        media_type.starts_with("image/")
            && (url.starts_with("https://") || url.starts_with("http://"))
    }
    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError> {
        let response = self.send(&options, false).await?;
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|error| ModelError::stream_error("读取 Chat Completions 响应失败", error))?;
        let mut result = parse_response(serde_json::from_slice(&body)?)?;
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
        Ok(chat_stream(
            response.bytes_stream(),
            ChatStreamParser::new(options.include_raw_chunks),
        ))
    }
}
