//! 会话：以只追加的条目日志为唯一事实来源，模型上下文由当前分支投影得出
//!
//! 压缩、回退、分叉都以追加条目表达，从不改写已写入的内容。设计见 `docs/session-persistence.md`。

mod entry;
mod log;
mod projection;
mod store;

pub use entry::{
    CompactionEntry, CompactionReason, ConfigEntry, CustomEntry, Entry, EntryBody, EntryId,
    ForkOrigin, MessageEntry, MessageOrigin, ModelRef, SessionHeader, SessionId, FORMAT_VERSION,
};
pub use log::{LogError, SessionLog};
pub use store::{BlobRef, MemoryStore, SessionStore, StoreError, StoredSession};
