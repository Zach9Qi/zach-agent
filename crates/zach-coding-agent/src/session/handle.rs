//! 会话句柄：创建、打开、发起运行、回退与分叉

use super::entry::{
    CompactionReason, ConfigEntry, CustomEntry, Entry, EntryBody, EntryId, ForkOrigin, ModelRef,
    SessionHeader, SessionId,
};
use super::error::SessionError;
use super::hooks::SessionHooks;
use super::log::SessionLog;
use super::options::SessionOptions;
use super::run::{spawn, SessionRun, Start};
use super::shared::{Recorder, Shared};
use super::store::SessionStore;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use zach_agent::Agent;
use zach_ai_core::Message;

/// 会话：驱动一个 [`Agent`]，把它写入对话记录的每条消息追加到会话日志
///
/// 会话占用传入 Agent 的对话记录接收方与钩子位（宿主钩子经 [`SessionOptions::with_hooks`] 传入）。
/// 发起运行请使用 [`Self::prompt`]，它会先以会话投影覆盖 Agent 的对话记录；
/// 插队、追加、中止与审批答复直接经 [`Self::agent`] 操作。句柄可廉价克隆。
#[derive(Clone)]
pub struct Session {
    pub(super) shared: Arc<Shared>,
    pub(super) agent: Agent,
}

impl Session {
    /// 新建会话；第一条消息写入时才在存储中创建
    pub fn create(store: Arc<dyn SessionStore>, agent: Agent, options: SessionOptions) -> Self {
        let header = SessionHeader::new(SessionId::generate(), options.workspace_root.clone());
        let shared = Shared::new(store, header, SessionLog::new(), false, options, Vec::new());
        Self::attach(shared, agent)
    }

    /// 打开已有会话，从其当前叶子继续
    pub async fn open(
        store: Arc<dyn SessionStore>,
        id: &SessionId,
        agent: Agent,
        options: SessionOptions,
    ) -> Result<Self, SessionError> {
        let stored = store.load(id).await?;
        let log = SessionLog::from_entries(stored.entries)?;
        let shared = Shared::new(store, stored.header, log, true, options, stored.warnings);
        Ok(Self::attach(shared, agent))
    }

    fn attach(shared: Shared, agent: Agent) -> Self {
        let shared = Arc::new(shared);
        agent.set_transcript(Some(Arc::new(Recorder {
            shared: shared.clone(),
        })));
        agent.set_hooks(SessionHooks {
            shared: shared.clone(),
            inner: shared.options.hooks.clone(),
        });
        Self { shared, agent }
    }

    pub fn id(&self) -> &SessionId {
        &self.shared.header.id
    }

    pub fn header(&self) -> &SessionHeader {
        &self.shared.header
    }

    /// 打开会话时存储报告的问题（如被跳过的残行）
    pub fn warnings(&self) -> &[String] {
        &self.shared.warnings
    }

    /// 被驱动的 Agent：用于插队、追加、中止与答复审批
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// 写入失败的原因；失败后会话拒绝新的运行与写入
    pub fn failure(&self) -> Option<String> {
        self.shared.failure()
    }

    /// 是否有运行在进行中
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// 以一批消息发起运行
    pub fn prompt(&self, messages: Vec<Message>) -> Result<SessionRun, SessionError> {
        self.start(Start::Prompt(messages))
    }

    /// 以一条用户文本发起运行
    pub fn prompt_text(&self, text: impl Into<String>) -> Result<SessionRun, SessionError> {
        self.prompt(vec![Message::user(text)])
    }

    /// 从当前叶子继续（如失败后重试）
    pub fn continue_run(&self) -> Result<SessionRun, SessionError> {
        self.start(Start::Continue)
    }

    fn start(&self, start: Start) -> Result<SessionRun, SessionError> {
        self.shared.ensure_healthy()?;
        let busy = self
            .shared
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err();
        if busy {
            return Err(SessionError::Busy);
        }
        Ok(spawn(self.clone(), start))
    }

    /// 全部条目，按写入顺序
    pub fn entries(&self) -> Vec<Entry> {
        self.shared.lock().log.entries().to_vec()
    }

    /// 当前叶子
    pub fn leaf(&self) -> Option<EntryId> {
        self.shared.lock().log.leaf()
    }

    /// 当前分支投影出的模型上下文（大对象已还原）
    pub async fn context(&self) -> Result<Vec<Message>, SessionError> {
        self.shared.context().await
    }

    /// 手动压缩：以 Agent 当前的模型生成摘要；没有可压缩的内容时返回 `Ok(None)`
    pub async fn compact(&self) -> Result<Option<EntryId>, SessionError> {
        if self.is_running() {
            return Err(SessionError::Busy);
        }
        let model = self.agent.model();
        self.shared.compact(&model, CompactionReason::Manual).await
    }

    /// 回退：把当前叶子移到 `to`，之后的运行从这里分出新分支，原分支保留
    pub async fn rewind(&self, to: EntryId) -> Result<(), SessionError> {
        if self.is_running() {
            return Err(SessionError::Busy);
        }
        self.shared
            .record(EntryBody::Checkout { target: to })
            .await?;
        Ok(())
    }

    /// 分叉：以根到 `at` 的分支创建新会话，交给 `agent` 驱动
    pub async fn fork(
        &self,
        at: EntryId,
        agent: Agent,
        options: SessionOptions,
    ) -> Result<Session, SessionError> {
        let entries: Vec<Entry> = {
            let state = self.shared.lock();
            let target = state.log.get(at).filter(|entry| entry.body.is_tree());
            if target.is_none() {
                return Err(super::log::LogError::InvalidCheckout { target: at }.into());
            }
            state.log.branch_to(at).into_iter().cloned().collect()
        };
        let mut header = SessionHeader::new(SessionId::generate(), options.workspace_root.clone());
        header.workspace_root = header
            .workspace_root
            .or_else(|| self.shared.header.workspace_root.clone());
        header.forked_from = Some(ForkOrigin {
            session: self.id().clone(),
            entry: at,
        });
        let log = SessionLog::from_entries(entries)?;
        let store = self.shared.store.clone();
        let shared = Shared::new(store, header, log, false, options, Vec::new());
        shared.flush().await?;
        Ok(Self::attach(shared, agent))
    }

    /// 追加宿主扩展条目（随分支生效，不进入上下文）
    pub async fn append_custom(
        &self,
        namespace: impl Into<String>,
        data: Value,
    ) -> Result<EntryId, SessionError> {
        let custom = CustomEntry {
            namespace: namespace.into(),
            data,
        };
        self.shared.record(EntryBody::Custom(custom)).await
    }

    /// Agent 的模型、系统提示词或工具与分支上记录的不同时，追加只含变化字段的配置条目
    pub(super) async fn record_config(&self) -> Result<(), SessionError> {
        let model = self.agent.model();
        let current = ConfigEntry {
            model: Some(ModelRef {
                provider: model.provider().to_string(),
                id: model.model_id().to_string(),
            }),
            system_prompt: self.agent.system_prompt(),
            tools: Some(
                self.agent
                    .tools()
                    .iter()
                    .map(|tool| tool.name().to_string())
                    .collect(),
            ),
        };
        let recorded = self.shared.lock().log.config();
        let change = ConfigEntry {
            model: current
                .model
                .filter(|model| recorded.model.as_ref() != Some(model)),
            system_prompt: current
                .system_prompt
                .filter(|prompt| recorded.system_prompt.as_ref() != Some(prompt)),
            tools: current
                .tools
                .filter(|tools| recorded.tools.as_ref() != Some(tools)),
        };
        if !change.is_empty() {
            self.shared.record(EntryBody::Config(change)).await?;
        }
        Ok(())
    }
}
