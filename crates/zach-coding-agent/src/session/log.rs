//! 会话日志：按写入顺序保存全部条目，按重放规则维护当前叶子

use super::entry::{ConfigEntry, Entry, EntryBody, EntryId};
use super::projection::project;
use std::collections::HashMap;
use std::fmt;
use zach_ai_core::Message;

/// 日志不变量被破坏
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    /// 条目 ID 没有按写入顺序递增
    NonIncreasingId { id: EntryId },
    /// 父条目不存在或不是更早的树条目
    MissingParent { id: EntryId, parent: EntryId },
    /// 回退目标不存在或不是树条目
    InvalidCheckout { target: EntryId },
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonIncreasingId { id } => write!(f, "条目 {id} 的 ID 没有递增"),
            Self::MissingParent { id, parent } => {
                write!(f, "条目 {id} 的父条目 {parent} 不存在")
            }
            Self::InvalidCheckout { target } => write!(f, "回退目标 {target} 不是已有的树条目"),
        }
    }
}

impl std::error::Error for LogError {}

/// 会话日志
///
/// 当前叶子由重放决定：树条目使叶子变为自身，`checkout` 使叶子变为其目标。
#[derive(Debug, Clone, Default)]
pub struct SessionLog {
    entries: Vec<Entry>,
    index: HashMap<EntryId, usize>,
    leaf: Option<EntryId>,
}

impl SessionLog {
    /// 空日志
    pub fn new() -> Self {
        Self::default()
    }

    /// 从已持久化的条目重建并校验不变量
    pub fn from_entries(entries: Vec<Entry>) -> Result<Self, LogError> {
        let mut log = Self::new();
        for entry in entries {
            log.validate(&entry)?;
            log.apply(entry);
        }
        Ok(log)
    }

    /// 追加新条目：分配下一个 ID，树条目的父条目取当前叶子
    pub fn append(&mut self, body: EntryBody, ts: u64) -> Result<&Entry, LogError> {
        let entry = Entry {
            id: self.next_id(),
            parent: if body.is_tree() { self.leaf } else { None },
            ts,
            body,
        };
        self.validate(&entry)?;
        self.apply(entry);
        Ok(self.entries.last().expect("刚刚追加"))
    }

    /// 全部条目，按写入顺序
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 按 ID 查找条目
    pub fn get(&self, id: EntryId) -> Option<&Entry> {
        self.index.get(&id).map(|&position| &self.entries[position])
    }

    /// 当前叶子
    pub fn leaf(&self) -> Option<EntryId> {
        self.leaf
    }

    /// 从根到 `leaf` 的树条目，按时间正序
    pub fn branch_to(&self, leaf: EntryId) -> Vec<&Entry> {
        let mut branch = Vec::new();
        let mut cursor = self.get(leaf);
        while let Some(entry) = cursor {
            branch.push(entry);
            cursor = entry.parent.and_then(|parent| self.get(parent));
        }
        branch.reverse();
        branch
    }

    /// 当前分支
    pub fn branch(&self) -> Vec<&Entry> {
        self.leaf
            .map(|leaf| self.branch_to(leaf))
            .unwrap_or_default()
    }

    /// 当前分支投影出的模型上下文
    pub fn context(&self) -> Vec<Message> {
        project(&self.branch())
    }

    /// 当前分支上逐字段合并后的配置
    pub fn config(&self) -> ConfigEntry {
        let mut config = ConfigEntry::default();
        for entry in self.branch() {
            if let EntryBody::Config(change) = &entry.body {
                config.merge(change);
            }
        }
        config
    }

    fn next_id(&self) -> EntryId {
        self.entries.last().map_or(1, |entry| entry.id + 1)
    }

    fn validate(&self, entry: &Entry) -> Result<(), LogError> {
        if self.entries.last().is_some_and(|last| entry.id <= last.id) {
            return Err(LogError::NonIncreasingId { id: entry.id });
        }
        if let Some(parent) = entry.parent {
            if !self.get(parent).is_some_and(|parent| parent.body.is_tree()) {
                return Err(LogError::MissingParent {
                    id: entry.id,
                    parent,
                });
            }
        }
        if let EntryBody::Checkout { target } = entry.body {
            if !self.get(target).is_some_and(|target| target.body.is_tree()) {
                return Err(LogError::InvalidCheckout { target });
            }
        }
        Ok(())
    }

    fn apply(&mut self, entry: Entry) {
        self.leaf = match entry.body {
            EntryBody::Checkout { target } => Some(target),
            _ => Some(entry.id),
        };
        self.index.insert(entry.id, self.entries.len());
        self.entries.push(entry);
    }
}
