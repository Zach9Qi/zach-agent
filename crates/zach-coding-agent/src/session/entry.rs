//! 会话条目：会话文件头与只追加日志中的各类条目
//!
//! 条目的 serde 形态即落盘格式，改动必须保持向后兼容或提升 [`FORMAT_VERSION`]，
//! 并由样例文件测试兜底（见 `docs/session-persistence.md`）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::time::{SystemTime, UNIX_EPOCH};
use zach_agent::{Commit, CommitKind};
use zach_ai_core::{FinishReason, Message, ReasoningEffort, Usage};

/// 当前写入的会话格式版本
pub const FORMAT_VERSION: u32 = 1;

/// 会话内的条目 ID，从 1 开始按写入顺序递增
pub type EntryId = u64;

/// 会话 ID：时间前缀（便于排序）加随机后缀
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// 生成新的会话 ID
    pub fn generate() -> Self {
        let random = RandomState::new().hash_one(SystemTime::now());
        Self(format!("{:012x}-{random:016x}", now_millis()))
    }

    /// 以已有字符串构造
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 会话文件头，序列化时带 `"type": "session"` 标记
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "session")]
pub struct SessionHeader {
    /// 写入时的格式版本
    pub format_version: u32,
    pub id: SessionId,
    /// 创建时间（Unix 毫秒）
    pub created_at: u64,
    /// 会话所属的工作区根目录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    /// 分叉来源
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
}

impl SessionHeader {
    /// 以当前格式版本与时间构造新会话的文件头
    pub fn new(id: SessionId, workspace_root: Option<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            id,
            created_at: now_millis(),
            workspace_root,
            forked_from: None,
        }
    }
}

/// 分叉来源：源会话及分叉点条目
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkOrigin {
    pub session: SessionId,
    pub entry: EntryId,
}

/// 日志中的一个条目
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    /// 树条目的父条目（写入时的当前叶子）；根条目与控制条目没有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<EntryId>,
    /// 写入时间（Unix 毫秒）
    pub ts: u64,
    #[serde(flatten)]
    pub body: EntryBody,
}

/// 条目内容
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryBody {
    /// 写入对话记录的消息
    Message(Box<MessageEntry>),
    /// 压缩摘要：替代分支上更早的消息
    Compaction(CompactionEntry),
    /// 运行配置的变化（只记录变化的字段），不进入上下文
    Config(ConfigEntry),
    /// 宿主扩展状态，随分支生效，不进入上下文
    Custom(CustomEntry),
    /// 控制条目：把当前叶子移到某个树条目（回退）
    Checkout { target: EntryId },
}

impl EntryBody {
    /// 消息条目
    pub fn message(entry: MessageEntry) -> Self {
        Self::Message(Box::new(entry))
    }

    /// 是否为树条目（参与分支结构）
    pub fn is_tree(&self) -> bool {
        !matches!(self, Self::Checkout { .. })
    }
}

/// 消息的来源，对应 [`CommitKind`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOrigin {
    Input,
    Response,
    ToolResults,
    Recovery,
}

/// 模型标识
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}

/// 消息条目：消息原样保存，附带信息放在消息之外
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageEntry {
    pub origin: MessageOrigin,
    pub message: Message,
    /// 生成该回复的模型
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// 该回复这一轮的用量
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// 工具给宿主的详情，按工具调用 ID
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_details: BTreeMap<String, Value>,
}

impl MessageEntry {
    /// 只有消息本身的条目
    pub fn new(origin: MessageOrigin, message: Message) -> Self {
        Self {
            origin,
            message,
            model: None,
            usage: None,
            finish_reason: None,
            tool_details: BTreeMap::new(),
        }
    }

    /// 由提交通知构造
    pub fn from_commit(commit: &Commit<'_>) -> Self {
        let message = commit.message.clone();
        match commit.kind {
            CommitKind::Input => Self::new(MessageOrigin::Input, message),
            CommitKind::Response(info) => Self {
                model: Some(ModelRef {
                    provider: info.provider.to_string(),
                    id: info.model_id.to_string(),
                }),
                usage: info.usage.cloned(),
                finish_reason: info.finish_reason.cloned(),
                ..Self::new(MessageOrigin::Response, message)
            },
            CommitKind::ToolResults { details } => Self {
                tool_details: details.iter().cloned().collect(),
                ..Self::new(MessageOrigin::ToolResults, message)
            },
            CommitKind::Recovery => Self::new(MessageOrigin::Recovery, message),
        }
    }
}

/// 触发压缩的原因
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    /// 手动触发
    Manual,
    /// 估算超过阈值
    Threshold,
    /// 模型报告上下文超长
    Overflow,
}

/// 压缩条目
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionEntry {
    /// 被压缩部分的摘要
    pub summary: String,
    /// 保留区起点：分支上从该条目起的消息原样保留；`None` 表示全部被压缩
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept: Option<EntryId>,
    /// 压缩前的估算 token 数
    pub tokens_before: u64,
    pub reason: CompactionReason,
    /// 至今读过的文件（含此前各次压缩）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_files: Vec<String>,
    /// 至今改过的文件（含此前各次压缩）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
}

/// 配置变化，只记录变化的字段；未设置的字段表示不变
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// 推理档位
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

impl ConfigEntry {
    /// 以 `later` 中出现的字段覆盖自身
    pub fn merge(&mut self, later: &ConfigEntry) {
        if later.model.is_some() {
            self.model.clone_from(&later.model);
        }
        if later.reasoning.is_some() {
            self.reasoning = later.reasoning;
        }
        if later.system_prompt.is_some() {
            self.system_prompt.clone_from(&later.system_prompt);
        }
        if later.tools.is_some() {
            self.tools.clone_from(&later.tools);
        }
    }

    /// 是否没有任何字段
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.reasoning.is_none()
            && self.system_prompt.is_none()
            && self.tools.is_none()
    }
}

/// 宿主扩展条目
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomEntry {
    /// 扩展的命名空间，避免不同扩展互相冲突
    pub namespace: String,
    pub data: Value,
}

/// 当前时间（Unix 毫秒）
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
