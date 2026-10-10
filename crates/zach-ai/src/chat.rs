//! OpenAI Chat Completions API 适配器。
//!
//! 该模块把 Chat Completions 的消息、工具和 SSE 分块转换为
//! `zach-ai-core` 的统一模型契约。端点默认为 `/v1/chat/completions`。

mod request;
mod response;
mod stream;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use std::fmt;
use std::sync::Arc;
use zach_ai_core::{
    CallOptions, GenerateResult, LanguageModel, LanguageModelStream, ModelError, ModelProfile,
};

use crate::transport;
use request::{build_request, BuiltRequest};
use response::parse_response;
use stream::{chat_stream, ChatStreamParser};

/// 厂商标识与错误信息中的协议名称。
const PROVIDER: &str = "openai";
const LABEL: &str = "OpenAI Chat Completions";

/// OpenAI Chat Completions 模型客户端。
#[derive(Clone)]
pub struct OpenAiChatCompletionsModel {
    client: Client,
    api_key: Arc<str>,
    base_url: Arc<str>,
    model_id: Arc<str>,
    default_headers: HeaderMap,
    /// 调用方注入的档案，优先于内置目录。
    profile: Option<Arc<ModelProfile>>,
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
            profile: None,
        }
    }

    /// 注入模型档案（能力、限制与计费），覆盖内置目录中的同名条目。
    ///
    /// 自定义端点、代理或内置目录尚未收录的模型据此获得 `max_tokens` 默认值等档案信息。
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = Some(Arc::new(profile));
        self
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
        let mut headers = self.default_headers.clone();
        // 空 Key 表示端点不需要鉴权（Ollama、vLLM 等本地服务），此时不发送 Authorization。
        if !self.api_key.trim().is_empty() {
            headers.insert(
                AUTHORIZATION,
                transport::sensitive_header(&format!("Bearer {}", self.api_key), "API Key")?,
            );
        }
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        transport::extend_headers(&mut headers, options.headers.as_ref())?;
        Ok(headers)
    }

    /// 构建请求，同时返回已序列化的正文供调试记录复用。
    fn request(
        &self,
        options: &CallOptions,
        stream: bool,
    ) -> Result<(reqwest::Request, BuiltRequest), ModelError> {
        let built = build_request(&self.model_id, self.profile(), options, stream)?;
        let request = self
            .client
            .post(self.endpoint())
            .headers(self.headers(options)?)
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
impl LanguageModel for OpenAiChatCompletionsModel {
    fn provider(&self) -> &str {
        PROVIDER
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn profile(&self) -> Option<&ModelProfile> {
        self.profile
            .as_deref()
            .or_else(|| crate::ModelCatalog::builtin().get(self.provider(), self.model_id()))
    }

    fn is_url_supported(&self, media_type: &str, url: &str) -> bool {
        // Chat Completions 的 `file` 内容块只接受 file_id 或内联数据，
        // PDF 等文件 URL 会在请求构建时被拒绝，这里必须保持一致。
        media_type.starts_with("image/")
            && (url.starts_with("https://") || url.starts_with("http://"))
    }

    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError> {
        let (response, built) = self.send(&options, false).await?;
        let (value, headers) = transport::read_json(response, LABEL).await?;
        let mut result = parse_response(value)?;
        result.request_body = Some(built.body);
        result.response_headers = Some(headers);
        result.warnings = built.warnings;
        Ok(result)
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let (response, built) = self.send(&options, true).await?;
        transport::require_event_stream(&response, PROVIDER)?;
        Ok(chat_stream(
            response.bytes_stream(),
            ChatStreamParser::new(options.include_raw_chunks),
            built.warnings,
        ))
    }
}
