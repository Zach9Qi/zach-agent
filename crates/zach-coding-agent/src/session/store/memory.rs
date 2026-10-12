//! 进程内存储：不落盘，适合测试与不需要持久化的宿主

use super::{BlobRef, SessionStore, StoreError, StoredSession};
use crate::session::entry::{Entry, SessionHeader, SessionId};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// 把会话与大对象保存在内存中的存储；进程退出即丢失
#[derive(Debug, Default)]
pub struct MemoryStore {
    sessions: Mutex<BTreeMap<SessionId, StoredSession>>,
    blobs: Mutex<HashMap<BlobRef, Vec<u8>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn sessions(&self) -> MutexGuard<'_, BTreeMap<SessionId, StoredSession>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn blobs(&self) -> MutexGuard<'_, HashMap<BlobRef, Vec<u8>>> {
        self.blobs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn create(&self, header: &SessionHeader) -> Result<(), StoreError> {
        let mut sessions = self.sessions();
        if sessions.contains_key(&header.id) {
            return Err(StoreError::AlreadyExists(header.id.clone()));
        }
        let session = StoredSession {
            header: header.clone(),
            entries: Vec::new(),
            warnings: Vec::new(),
        };
        sessions.insert(header.id.clone(), session);
        Ok(())
    }

    async fn append(&self, id: &SessionId, entries: &[Entry]) -> Result<(), StoreError> {
        let mut sessions = self.sessions();
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(id.clone()))?;
        session.entries.extend_from_slice(entries);
        Ok(())
    }

    async fn sync(&self, id: &SessionId) -> Result<(), StoreError> {
        if self.sessions().contains_key(id) {
            Ok(())
        } else {
            Err(StoreError::NotFound(id.clone()))
        }
    }

    async fn load(&self, id: &SessionId) -> Result<StoredSession, StoreError> {
        self.sessions()
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.clone()))
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, StoreError> {
        let mut headers: Vec<SessionHeader> = self
            .sessions()
            .values()
            .map(|session| session.header.clone())
            .collect();
        headers.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(headers)
    }

    async fn put_blob(&self, blob: &BlobRef, data: &[u8]) -> Result<(), StoreError> {
        self.blobs()
            .entry(blob.clone())
            .or_insert_with(|| data.to_vec());
        Ok(())
    }

    async fn get_blob(&self, blob: &BlobRef) -> Result<Vec<u8>, StoreError> {
        self.blobs()
            .get(blob)
            .cloned()
            .ok_or_else(|| StoreError::BlobNotFound(blob.clone()))
    }
}
