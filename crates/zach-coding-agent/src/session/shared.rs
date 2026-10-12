//! 会话共享状态：内存中的日志、落盘进度与失败标记，以及接收提交通知的记录器
//!
//! 写入规则：
//! - 条目先进入内存日志，再按顺序追加到存储；同一时刻只有一个写入在进行。
//! - 懒创建：日志中出现第一条消息时才在存储中创建会话，此前的条目（如配置）随之一起写入。
//! - 任一写入失败即标记失败：钩子随即停止执行工具并结束运行，此后的写入一律拒绝。

use super::blobs::{externalize, internalize};
use super::entry::{now_millis, Entry, EntryBody, EntryId, MessageEntry, SessionHeader};
use super::error::SessionError;
use super::log::SessionLog;
use super::options::{Durability, SessionOptions};
use super::run::SessionEvent;
use super::store::SessionStore;
use async_trait::async_trait;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::mpsc;
use zach_agent::{Commit, CommitKind, TranscriptSink};
use zach_ai_core::{AssistantPart, Message};

pub(super) struct State {
    pub(super) log: SessionLog,
    /// 已追加到存储的条目数（日志前缀）
    persisted: usize,
    /// 存储中是否已创建该会话
    created: bool,
    failure: Option<String>,
}

pub(super) struct Shared {
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) header: SessionHeader,
    pub(super) options: SessionOptions,
    pub(super) warnings: Vec<String>,
    /// 会话是否有运行在进行中
    pub(super) running: AtomicBool,
    state: Mutex<State>,
    /// 串行化写入，保证追加顺序
    writer: tokio::sync::Mutex<()>,
    /// 当前运行的会话事件通道（压缩事件经此送达使用方）
    events: Mutex<Option<mpsc::UnboundedSender<SessionEvent>>>,
}

impl Shared {
    /// `created` 表示日志中的条目已全部在存储中
    pub(super) fn new(
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        log: SessionLog,
        created: bool,
        options: SessionOptions,
        warnings: Vec<String>,
    ) -> Self {
        let persisted = if created { log.entries().len() } else { 0 };
        Self {
            store,
            header,
            options,
            warnings,
            running: AtomicBool::new(false),
            state: Mutex::new(State {
                log,
                persisted,
                created,
                failure: None,
            }),
            writer: tokio::sync::Mutex::new(()),
            events: Mutex::new(None),
        }
    }

    /// 设置或清除当前运行的会话事件通道
    pub(super) fn set_events(&self, sender: Option<mpsc::UnboundedSender<SessionEvent>>) {
        *self.events.lock().unwrap_or_else(PoisonError::into_inner) = sender;
    }

    /// 向当前运行的使用方发送会话事件；没有运行时丢弃
    pub(super) fn send_event(&self, event: SessionEvent) {
        let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = events.as_ref() {
            let _ = sender.send(event);
        }
    }

    /// 当前分支投影出的模型上下文，大对象已还原
    pub(super) async fn context(&self) -> Result<Vec<Message>, SessionError> {
        let mut messages = self.lock().log.context();
        for message in &mut messages {
            internalize(message, self.store.as_ref()).await?;
        }
        Ok(messages)
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 写入失败的原因；会话一旦失败就不再写入
    pub(super) fn failure(&self) -> Option<String> {
        self.lock().failure.clone()
    }

    pub(super) fn ensure_healthy(&self) -> Result<(), SessionError> {
        self.failure()
            .map_or(Ok(()), |failure| Err(SessionError::Persist(failure)))
    }

    /// 追加条目到日志并落盘，返回其 ID
    pub(super) async fn record(&self, body: EntryBody) -> Result<EntryId, SessionError> {
        self.ensure_healthy()?;
        let id = self.lock().log.append(body, now_millis())?.id;
        self.flush().await?;
        Ok(id)
    }

    /// 把尚未落盘的条目写入存储（满足懒创建条件时）
    pub(super) async fn flush(&self) -> Result<(), SessionError> {
        let _writer = self.writer.lock().await;
        let (mut pending, create) = {
            let state = self.lock();
            if let Some(failure) = &state.failure {
                return Err(SessionError::Persist(failure.clone()));
            }
            let has_message = state.created
                || state
                    .log
                    .entries()
                    .iter()
                    .any(|entry| matches!(entry.body, EntryBody::Message(_)));
            if !has_message {
                return Ok(());
            }
            (
                state.log.entries()[state.persisted..].to_vec(),
                !state.created,
            )
        };
        if pending.is_empty() && !create {
            return Ok(());
        }
        match self.write(&mut pending, create).await {
            Ok(()) => {
                let mut state = self.lock();
                state.persisted += pending.len();
                state.created = true;
                Ok(())
            }
            Err(error) => Err(self.fail(error)),
        }
    }

    /// 确保已追加的内容落到持久介质；会话尚未创建时什么也不做
    pub(super) async fn sync(&self) -> Result<(), SessionError> {
        if !self.lock().created {
            return Ok(());
        }
        let _writer = self.writer.lock().await;
        self.store
            .sync(&self.header.id)
            .await
            .map_err(|error| self.fail(error.into()))
    }

    async fn write(&self, pending: &mut [Entry], create: bool) -> Result<(), SessionError> {
        for entry in pending.iter_mut() {
            if let EntryBody::Message(message) = &mut entry.body {
                let threshold = self.options.blob_threshold;
                externalize(&mut message.message, self.store.as_ref(), threshold).await?;
            }
        }
        if create {
            self.store.create(&self.header).await?;
        }
        if !pending.is_empty() {
            self.store.append(&self.header.id, pending).await?;
        }
        Ok(())
    }

    /// 标记失败并返回对外的错误
    fn fail(&self, error: SessionError) -> SessionError {
        let reason = error.to_string();
        tracing::error!(session = %self.header.id, %reason, "会话写入失败");
        self.lock().failure.get_or_insert(reason.clone());
        SessionError::Persist(reason)
    }
}

/// 接收 Agent 的提交通知，逐条写入会话
pub(super) struct Recorder {
    pub(super) shared: Arc<Shared>,
}

#[async_trait]
impl TranscriptSink for Recorder {
    async fn on_commit(&self, commit: &Commit<'_>) {
        let body = EntryBody::message(MessageEntry::from_commit(commit));
        // 失败已记录在共享状态中，由钩子停止运行
        if self.shared.record(body).await.is_err() {
            return;
        }
        let sync = match self.shared.options.durability {
            Durability::EveryEntry => true,
            Durability::TurnBoundary => ends_turn(commit),
            Durability::OsDefault => false,
        };
        if sync {
            let _ = self.shared.sync().await;
        }
    }
}

/// 一轮是否已完整写入：工具结果、补齐结果，或不再发起本地工具调用的回复
fn ends_turn(commit: &Commit<'_>) -> bool {
    match commit.kind {
        CommitKind::Input => false,
        CommitKind::ToolResults { .. } | CommitKind::Recovery => true,
        CommitKind::Response(_) => !has_local_calls(commit.message),
    }
}

fn has_local_calls(message: &Message) -> bool {
    let Message::Assistant { content, .. } = message else {
        return false;
    };
    content.iter().any(|part| {
        matches!(
            part,
            AssistantPart::ToolCall {
                provider_executed: false,
                ..
            }
        )
    })
}
