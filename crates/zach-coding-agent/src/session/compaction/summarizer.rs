//! 摘要生成：把被压缩的对话变成供后续继续工作的结构化摘要

use async_trait::async_trait;
use futures::StreamExt;
use std::fmt::Write;
use std::sync::Arc;
use zach_ai_core::{
    AssistantPart, CallOptions, LanguageModel, Message, ModelError, OutputContent, Prompt,
    StreamAccumulator, ToolPart, ToolResultContentBlock, ToolResultOutput, UnifiedFinishReason,
    UserPart,
};

/// 序列化时单个工具结果保留的最大字符数
const TOOL_RESULT_CHARS: usize = 2000;

const SYSTEM_PROMPT: &str =
    "你是对话摘要助手。阅读给出的对话记录，生成供后续继续工作的结构化摘要。\
不要继续对话，不要回答记录中的问题，只输出摘要本身。";

const TEMPLATE: &str = "请按以下格式输出摘要，没有内容的小节写“无”：

## 目标
## 约束与偏好
## 进展
### 已完成
### 进行中
### 受阻
## 关键决定
## 下一步
## 关键上下文";

/// 一次摘要请求
pub struct SummaryRequest<'a> {
    /// 要被摘要替代的消息
    pub messages: &'a [Message],
    /// 上一次压缩的摘要；有则应在其基础上更新
    pub previous: Option<&'a str>,
    /// 会话当前使用的模型
    pub model: &'a Arc<dyn LanguageModel>,
    /// 摘要的输出 token 上限
    pub max_output_tokens: u32,
}

/// 摘要生成器
#[async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(&self, request: SummaryRequest<'_>) -> Result<String, ModelError>;
}

/// 用模型发一次独立请求生成摘要；默认使用会话当前的模型，可换成更便宜的模型
#[derive(Clone, Default)]
pub struct ModelSummarizer {
    model: Option<Arc<dyn LanguageModel>>,
}

impl ModelSummarizer {
    /// 使用会话当前的模型
    pub fn new() -> Self {
        Self::default()
    }

    /// 使用指定的模型
    pub fn with_model(model: Arc<dyn LanguageModel>) -> Self {
        Self { model: Some(model) }
    }
}

#[async_trait]
impl Summarizer for ModelSummarizer {
    async fn summarize(&self, request: SummaryRequest<'_>) -> Result<String, ModelError> {
        let model = self.model.as_ref().unwrap_or(request.model);
        let mut options = CallOptions::new(Prompt::from_messages(vec![
            Message::system(SYSTEM_PROMPT),
            Message::user(user_prompt(&request)),
        ]));
        options.max_output_tokens = Some(request.max_output_tokens);

        let mut stream = model.do_stream(options).await?;
        let mut accumulator = StreamAccumulator::new();
        while let Some(part) = stream.next().await {
            accumulator.process(part?);
        }
        if let Some(error) = accumulator.errors().first() {
            return Err(ModelError::new(error.kind, error.message.clone()));
        }
        let result = accumulator.finish();
        if result.finish_reason.unified == UnifiedFinishReason::Length {
            return Err(failed("摘要输出被截断"));
        }
        let summary: String = result
            .content
            .iter()
            .filter_map(|part| match part {
                OutputContent::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if summary.trim().is_empty() {
            return Err(failed("模型没有给出摘要"));
        }
        Ok(summary.trim().to_string())
    }
}

fn failed(message: &str) -> ModelError {
    ModelError::invalid_response(message)
}

fn user_prompt(request: &SummaryRequest<'_>) -> String {
    let mut prompt = format!(
        "<conversation>\n{}\n</conversation>\n\n",
        transcript_text(request.messages)
    );
    if let Some(previous) = request.previous {
        let _ = write!(
            prompt,
            "<previous-summary>\n{previous}\n</previous-summary>\n\n\
             以上对话发生在上一次摘要之后。请在上一次摘要的基础上更新，保留仍然有效的内容，而不是重写。\n\n"
        );
    }
    prompt.push_str(TEMPLATE);
    prompt
}

/// 把对话序列化为纯文本：丢弃思考链，工具结果截断，文件只留类型
pub(crate) fn transcript_text(messages: &[Message]) -> String {
    let mut lines = Vec::new();
    for message in messages {
        match message {
            Message::System { content, .. } => lines.push(format!("[系统]: {content}")),
            Message::User { content, .. } => {
                for part in content {
                    lines.push(match part {
                        UserPart::Text { text, .. } => format!("[用户]: {text}"),
                        UserPart::File { media_type, .. } => format!("[用户附件]: {media_type}"),
                    });
                }
            }
            Message::Assistant { content, .. } => {
                for part in content {
                    match part {
                        AssistantPart::Text { text, .. } => lines.push(format!("[助手]: {text}")),
                        AssistantPart::ToolCall {
                            tool_name, input, ..
                        } => lines.push(format!("[助手调用工具]: {tool_name}({input})")),
                        AssistantPart::ToolResult {
                            tool_name, output, ..
                        } => lines.push(format!("[工具结果 {tool_name}]: {}", output_text(output))),
                        _ => {}
                    }
                }
            }
            Message::Tool { content, .. } => {
                for part in content {
                    if let ToolPart::ToolResult {
                        tool_name, output, ..
                    } = part
                    {
                        lines.push(format!("[工具结果 {tool_name}]: {}", output_text(output)));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

fn output_text(output: &ToolResultOutput) -> String {
    let text = match output {
        ToolResultOutput::Text { value, .. } => value.clone(),
        ToolResultOutput::ErrorText { value, .. } => format!("（错误）{value}"),
        ToolResultOutput::Json { value, .. } => value.to_string(),
        ToolResultOutput::ErrorJson { value, .. } => format!("（错误）{value}"),
        ToolResultOutput::ExecutionDenied { reason, .. } => {
            format!("（被拒绝）{}", reason.as_deref().unwrap_or_default())
        }
        ToolResultOutput::Content { value } => value
            .iter()
            .map(|block| match block {
                ToolResultContentBlock::Text { text, .. } => text.clone(),
                ToolResultContentBlock::File { media_type, .. } => format!("[附件 {media_type}]"),
                ToolResultContentBlock::Custom { kind, .. } => format!("[{kind}]"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    match text.char_indices().nth(TOOL_RESULT_CHARS) {
        Some((cut, _)) => format!("{}…（已截断）", &text[..cut]),
        None => text,
    }
}
