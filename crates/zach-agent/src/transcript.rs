//! 对话记录的提交通知：每条消息写入对话记录时，连同其附带信息一起告知宿主
//!
//! 事件流面向界面、只尽力投递，且没有完整消息；需要持久化或审计对话记录时应使用这里的通知：
//! 循环按写入顺序逐条调用并等待其返回，运行被中止时也不会跳过。

use async_trait::async_trait;
use serde_json::Value;
use zach_ai_core::{FinishReason, Message, Usage};

/// 一条写入对话记录的消息及其附带信息
#[derive(Debug, Clone, Copy)]
pub struct Commit<'a> {
    /// 写入的消息
    pub message: &'a Message,
    /// 消息的来源与附带信息
    pub kind: CommitKind<'a>,
}

/// 提交的来源与附带信息
#[derive(Debug, Clone, Copy)]
pub enum CommitKind<'a> {
    /// 初始提示、插队/追加消息、钩子注入的消息
    Input,
    /// 模型回复
    Response(ResponseInfo<'a>),
    /// 工具结果；中止时未完成的调用以"已中止"错误结果出现在这里
    ToolResults {
        /// 各调用给宿主的详情（[`crate::ToolOutcome::details`]），按 `(tool_call_id, details)` 列出，没有详情的调用不出现
        details: &'a [(String, Value)],
    },
    /// 运行任务异常（panic）后为悬空工具调用补齐的结果
    Recovery,
}

/// 模型回复的附带信息
#[derive(Debug, Clone, Copy)]
pub struct ResponseInfo<'a> {
    /// 生成该回复的模型厂商
    pub provider: &'a str,
    /// 生成该回复的模型 ID
    pub model_id: &'a str,
    /// 本轮用量；回复因中止而只有部分内容时为 `None`
    pub usage: Option<&'a Usage>,
    /// 结束原因；回复因中止而只有部分内容时为 `None`
    pub finish_reason: Option<&'a FinishReason>,
}

/// 对话记录的接收方，经 [`crate::AgentBuilder::transcript`] 接入 [`crate::Agent`]
///
/// 循环等待回调返回后才继续，回调顺序即对话记录顺序；与 [`crate::AgentHooks`] 不同，
/// 运行被中止时回调不会被丢弃。实现方不返回错误：处理失败（如落盘失败）时应自行兜底，
/// 例如记录错误并中止当前运行。
#[async_trait]
pub trait TranscriptSink: Send + Sync {
    /// 一条消息写入了对话记录
    async fn on_commit(&self, commit: &Commit<'_>);
}
