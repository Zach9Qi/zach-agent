//! 会话选项

use super::compaction::CompactionOptions;
use std::fmt;
use std::sync::Arc;
use zach_agent::{AgentHooks, NoopHooks};

/// 大对象外置的默认阈值：超过它的文件数据单独存放
const DEFAULT_BLOB_THRESHOLD: usize = 32 * 1024;

/// 刷盘策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Durability {
    /// 每个条目写入后都刷盘
    EveryEntry,
    /// 一轮（回复与其工具结果）完整写入后刷盘
    #[default]
    TurnBoundary,
    /// 不主动刷盘，由操作系统决定
    OsDefault,
}

/// 会话选项
#[derive(Clone)]
pub struct SessionOptions {
    pub(crate) workspace_root: Option<String>,
    pub(crate) hooks: Arc<dyn AgentHooks>,
    pub(crate) durability: Durability,
    pub(crate) blob_threshold: usize,
    pub(crate) compaction: CompactionOptions,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            workspace_root: None,
            hooks: Arc::new(NoopHooks),
            durability: Durability::default(),
            blob_threshold: DEFAULT_BLOB_THRESHOLD,
            compaction: CompactionOptions::default(),
        }
    }
}

impl SessionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 新会话所属的工作区根目录，写入会话文件头
    pub fn with_workspace_root(mut self, root: impl Into<String>) -> Self {
        self.workspace_root = Some(root.into());
        self
    }

    /// 宿主自己的策略钩子
    ///
    /// 会话占用 `Agent` 的钩子位，宿主钩子经这里传入，由会话串联调用。
    pub fn with_hooks(mut self, hooks: impl AgentHooks + 'static) -> Self {
        self.hooks = Arc::new(hooks);
        self
    }

    /// 刷盘策略，默认 [`Durability::TurnBoundary`]
    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// 大对象外置阈值（字节），默认 32KB
    pub fn with_blob_threshold(mut self, bytes: usize) -> Self {
        self.blob_threshold = bytes;
        self
    }

    /// 上下文压缩选项
    pub fn with_compaction(mut self, compaction: CompactionOptions) -> Self {
        self.compaction = compaction;
        self
    }
}

impl fmt::Debug for SessionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionOptions")
            .field("workspace_root", &self.workspace_root)
            .field("durability", &self.durability)
            .field("blob_threshold", &self.blob_threshold)
            .field("compaction", &self.compaction)
            .finish_non_exhaustive()
    }
}
