//! 写入对话记录：同步通知宿主，再更新本次请求的上下文与运行结果

use super::Runner;
use crate::transcript::{Commit, CommitKind, ResponseInfo};
use zach_ai_core::{FinishReason, Message, Usage};

impl Runner<'_> {
    pub(super) async fn commit(&mut self, message: Message, kind: CommitKind<'_>) {
        let commit = Commit {
            message: &message,
            kind,
        };
        self.host.on_commit(&commit).await;
        self.state.context.messages.push(message.clone());
        self.messages.push(message);
    }

    /// 写入模型回复；`done` 为本轮用量与结束原因，中止时只有部分内容则为 `None`
    pub(super) async fn commit_response(
        &mut self,
        message: Message,
        done: Option<(&Usage, &FinishReason)>,
    ) {
        let provider = self.state.model.provider().to_string();
        let model_id = self.state.model.model_id().to_string();
        let info = ResponseInfo {
            provider: &provider,
            model_id: &model_id,
            usage: done.map(|(usage, _)| usage),
            finish_reason: done.map(|(_, reason)| reason),
        };
        self.commit(message, CommitKind::Response(info)).await;
    }
}
