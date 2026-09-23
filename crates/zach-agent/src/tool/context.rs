//! 单次工具调用的执行上下文

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zach_ai_core::ToolResultOutput;

/// 传给 [`crate::AgentTool::execute`] 的执行上下文
///
/// 只在本次 `execute` 期间有效：执行结束后再上报的进度会被丢弃。
#[derive(Debug, Clone)]
pub struct ToolContext {
    tool_call_id: String,
    cancel: CancellationToken,
    progress: Option<mpsc::UnboundedSender<ToolResultOutput>>,
}

impl ToolContext {
    /// 构造不带进度通道的上下文（便于单独测试工具）
    pub fn new(tool_call_id: impl Into<String>, cancel: CancellationToken) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            cancel,
            progress: None,
        }
    }

    pub(crate) fn with_progress(mut self, sender: mpsc::UnboundedSender<ToolResultOutput>) -> Self {
        self.progress = Some(sender);
        self
    }

    /// 本次工具调用 ID
    pub fn tool_call_id(&self) -> &str {
        &self.tool_call_id
    }

    /// 运行级取消令牌。长耗时工具应监听它并尽快返回。
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// 运行是否已被中止
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// 上报中间态结果，循环会以 `preliminary = true` 的 `ToolOutputAvailable` 事件透出
    pub fn report_progress(&self, output: ToolResultOutput) {
        if let Some(sender) = &self.progress {
            let _ = sender.send(output);
        }
    }
}
