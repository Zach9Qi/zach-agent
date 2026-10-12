//! 会话：以只追加的条目日志为唯一事实来源，模型上下文由当前分支投影得出
//!
//! 压缩、回退、分叉都以追加条目表达，从不改写已写入的内容。设计见 `docs/session-persistence.md`。

mod blobs;
mod entry;
mod error;
mod handle;
mod hooks;
mod log;
mod options;
mod projection;
mod run;
mod shared;
mod store;

pub use entry::{
    CompactionEntry, CompactionReason, ConfigEntry, CustomEntry, Entry, EntryBody, EntryId,
    ForkOrigin, MessageEntry, MessageOrigin, ModelRef, SessionHeader, SessionId, FORMAT_VERSION,
};
pub use error::SessionError;
pub use handle::Session;
pub use log::{LogError, SessionLog};
pub use options::{Durability, SessionOptions};
pub use run::{SessionEvent, SessionRun};
pub use store::{BlobRef, MemoryStore, SessionStore, StoreError, StoredSession};
