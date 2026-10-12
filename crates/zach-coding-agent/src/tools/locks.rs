//! 按文件的互斥锁：防止并行工具对同一文件"读-改-写"时互相覆盖

use crate::workspace::path_key;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// 以规范化路径为键的锁表；无人持有或等待的锁会被自动回收
#[derive(Debug, Default)]
pub(crate) struct FileLocks {
    slots: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl FileLocks {
    /// 获取某文件的锁，同一文件按调用顺序排队（tokio 互斥锁保证先到先得）
    pub(crate) async fn lock(&self, path: &Path) -> OwnedMutexGuard<()> {
        let mutex = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            slots.retain(|_, slot| slot.strong_count() > 0);
            let key = path_key(path);
            match slots.get(&key).and_then(Weak::upgrade) {
                Some(mutex) => mutex,
                None => {
                    let mutex = Arc::new(AsyncMutex::new(()));
                    slots.insert(key, Arc::downgrade(&mutex));
                    mutex
                }
            }
        };
        mutex.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;

    #[tokio::test]
    async fn same_file_waits_while_other_files_proceed() {
        let locks = FileLocks::default();
        let held = locks.lock(Path::new("/ws/a.rs")).await;

        assert!(locks.lock(Path::new("/ws/a.rs")).now_or_never().is_none());
        assert!(locks.lock(Path::new("/ws/b.rs")).now_or_never().is_some());

        drop(held);
        assert!(locks.lock(Path::new("/ws/a.rs")).now_or_never().is_some());
    }

    #[tokio::test]
    async fn released_locks_are_pruned() {
        let locks = FileLocks::default();
        drop(locks.lock(Path::new("/ws/a.rs")).await);
        drop(locks.lock(Path::new("/ws/b.rs")).await);
        assert!(locks.slots.lock().unwrap().len() <= 1);
    }
}
