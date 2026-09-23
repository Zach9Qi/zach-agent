//! 统一语言模型（LanguageModel）Trait 抽象契约

use crate::call_options::CallOptions;
use crate::error::ModelError;
use crate::response::GenerateResult;
use crate::stream::StreamPart;
use async_trait::async_trait;
use futures_core::Stream;
use std::pin::Pin;

/// 流式生成返回的异步流类型别名
///
/// 错误分三层：
/// - `do_stream` 返回 `Err`：流建立前失败，尚无输出，可安全重试；
/// - `Ok(StreamPart::Error)`：厂商错误事件或单个分块解析失败，不终止流，可多次出现；
/// - `Err(ModelError)`：传输中断等不可恢复错误，产出后流必须立即结束。
pub type LanguageModelStream =
    Pin<Box<dyn Stream<Item = Result<StreamPart, ModelError>> + Send + 'static>>;

/// 语言模型标准抽象接口契约。
/// 所有底层厂商适配层（OpenAI, Anthropic, Gemini, 本地 Ollama 等）均实现此接口。
#[async_trait]
pub trait LanguageModel: Send + Sync {
    /// 规范版本（固定为 "v4"）
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// 厂商唯一标识（如 "openai", "anthropic", "google", "ollama"）
    fn provider(&self) -> &str;

    /// 模型唯一标识（如 "gpt-4o", "claude-3-7-sonnet"）
    fn model_id(&self) -> &str;

    /// 判断该模型是否原生支持指定媒体类型的直传 URL（无需客户端先下载为字节）
    fn is_url_supported(&self, media_type: &str, url: &str) -> bool {
        let _ = (media_type, url);
        false
    }

    /// 非流式生成
    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError>;

    /// 流式生成
    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError>;
}
