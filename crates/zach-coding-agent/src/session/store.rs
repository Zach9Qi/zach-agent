//! 会话存储：会话层只经由 [`SessionStore`] 读写，宿主可接入文件、数据库或远程实现

mod memory;

pub use memory::MemoryStore;

use super::entry::{Entry, SessionHeader, SessionId};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;

/// 存储错误
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("会话 {0} 不存在")]
    NotFound(SessionId),
    #[error("会话 {0} 已存在")]
    AlreadyExists(SessionId),
    #[error("会话 ID {0:?} 含有不允许的字符")]
    InvalidId(SessionId),
    #[error("会话 {id} 第 {line} 行损坏: {reason}")]
    Corrupt {
        id: SessionId,
        line: usize,
        reason: String,
    },
    #[error("会话 {id} 的格式版本 {version} 高于当前支持的 {supported}")]
    UnsupportedVersion {
        id: SessionId,
        version: u32,
        supported: u32,
    },
    #[error("blob {0} 不存在")]
    BlobNotFound(BlobRef),
    #[error("存储读写失败: {0}")]
    Io(#[from] io::Error),
}

/// 已持久化的会话
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSession {
    pub header: SessionHeader,
    /// 全部条目，按写入顺序
    pub entries: Vec<Entry>,
    /// 加载时被跳过的问题（如崩溃留下的末尾残行）
    pub warnings: Vec<String>,
}

/// 大对象引用：内容的 SHA-256，形如 `sha256-<hex>`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobRef(String);

impl BlobRef {
    /// 计算内容的引用
    pub fn of(data: &[u8]) -> Self {
        Self(format!("sha256-{:x}", Sha256::digest(data)))
    }

    /// 解析引用字符串，格式不对时返回 `None`
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.strip_prefix("sha256-")?;
        let valid = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        valid.then(|| Self(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 会话存储
///
/// 同一会话同时只有一个写入方（由会话层保证）；`append` 必须把一批条目完整写入或报错。
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// 以文件头创建会话；已存在时返回 [`StoreError::AlreadyExists`]
    async fn create(&self, header: &SessionHeader) -> Result<(), StoreError>;

    /// 追加条目
    async fn append(&self, id: &SessionId, entries: &[Entry]) -> Result<(), StoreError>;

    /// 确保已追加的内容落到持久介质
    async fn sync(&self, id: &SessionId) -> Result<(), StoreError>;

    /// 加载会话
    async fn load(&self, id: &SessionId) -> Result<StoredSession, StoreError>;

    /// 列出全部会话的文件头，最新的在前；无法读取文件头的会话被跳过
    async fn list(&self) -> Result<Vec<SessionHeader>, StoreError>;

    /// 保存大对象；已存在时不做任何事
    async fn put_blob(&self, blob: &BlobRef, data: &[u8]) -> Result<(), StoreError>;

    /// 读取大对象
    async fn get_blob(&self, blob: &BlobRef) -> Result<Vec<u8>, StoreError>;
}
