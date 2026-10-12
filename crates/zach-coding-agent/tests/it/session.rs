//! 会话：条目日志与投影、v1 样例文件、存储契约与运行生命周期；子模块共用的辅助放在这里

mod fixtures;
mod lifecycle;
mod log;
mod store;

use async_trait::async_trait;
use serde_json::json;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use zach_ai_core::{AssistantPart, Message, ToolPart, ToolResultOutput};
use zach_coding_agent::session::{
    BlobRef, Entry, EntryBody, MemoryStore, MessageEntry, MessageOrigin, SessionHeader, SessionId,
    SessionStore, StoreError, StoredSession,
};

/// 用户输入条目
fn input(text: &str) -> EntryBody {
    EntryBody::message(MessageEntry::new(MessageOrigin::Input, Message::user(text)))
}

/// 纯文本回复条目
fn reply(text: &str) -> EntryBody {
    EntryBody::message(MessageEntry::new(
        MessageOrigin::Response,
        Message::assistant(text),
    ))
}

/// 发起若干本地工具调用的回复条目
fn calls(ids: &[&str]) -> EntryBody {
    let content = ids
        .iter()
        .map(|id| AssistantPart::ToolCall {
            tool_call_id: (*id).into(),
            tool_name: "read".into(),
            input: json!({}),
            provider_executed: false,
            provider_options: None,
        })
        .collect();
    EntryBody::message(MessageEntry::new(
        MessageOrigin::Response,
        Message::Assistant {
            content,
            provider_options: None,
        },
    ))
}

/// 若干工具调用的文本结果条目
fn results(ids: &[&str]) -> EntryBody {
    let parts = ids.iter().map(|id| result_part(id, "ok")).collect();
    EntryBody::message(MessageEntry::new(
        MessageOrigin::ToolResults,
        Message::tool(parts),
    ))
}

fn result_part(id: &str, text: &str) -> ToolPart {
    ToolPart::ToolResult {
        tool_call_id: id.into(),
        tool_name: "read".into(),
        output: ToolResultOutput::text(text),
        provider_options: None,
    }
}

/// 工具消息中各结果的 `(tool_call_id, 文本)`
fn tool_outputs(message: &Message) -> Vec<(String, String)> {
    let Message::Tool { content, .. } = message else {
        panic!("不是工具消息: {message:?}");
    };
    content
        .iter()
        .map(|part| match part {
            ToolPart::ToolResult {
                tool_call_id,
                output: ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. },
                ..
            } => (tool_call_id.clone(), value.clone()),
            other => panic!("意外的工具结果: {other:?}"),
        })
        .collect()
}

/// 前 `ok_appends` 次追加成功、之后一律失败的存储，模拟磁盘写满
struct FailingStore {
    inner: MemoryStore,
    ok_appends: usize,
    appends: AtomicUsize,
}

impl FailingStore {
    fn new(ok_appends: usize) -> Self {
        Self {
            inner: MemoryStore::new(),
            ok_appends,
            appends: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SessionStore for FailingStore {
    async fn create(&self, header: &SessionHeader) -> Result<(), StoreError> {
        self.inner.create(header).await
    }

    async fn append(&self, id: &SessionId, entries: &[Entry]) -> Result<(), StoreError> {
        if self.appends.fetch_add(1, Ordering::SeqCst) >= self.ok_appends {
            return Err(io::Error::other("磁盘已满").into());
        }
        self.inner.append(id, entries).await
    }

    async fn sync(&self, id: &SessionId) -> Result<(), StoreError> {
        self.inner.sync(id).await
    }

    async fn load(&self, id: &SessionId) -> Result<StoredSession, StoreError> {
        self.inner.load(id).await
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, StoreError> {
        self.inner.list().await
    }

    async fn put_blob(&self, blob: &BlobRef, data: &[u8]) -> Result<(), StoreError> {
        self.inner.put_blob(blob, data).await
    }

    async fn get_blob(&self, blob: &BlobRef) -> Result<Vec<u8>, StoreError> {
        self.inner.get_blob(blob).await
    }
}
