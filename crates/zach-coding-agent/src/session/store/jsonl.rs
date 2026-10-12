//! 文件存储：一个会话一个 JSONL 文件，大对象按内容寻址单独存放
//!
//! 目录结构：`<root>/sessions/<id>.jsonl` 与 `<root>/blobs/sha256-<hex>`，根目录由宿主指定。
//! 打开已有会话追加前先截掉末尾残行，避免新条目接在残行后面；这是唯一的非追加写。

mod disk;
mod format;

use super::{BlobRef, SessionStore, StoreError, StoredSession};
use crate::session::entry::{Entry, SessionHeader, SessionId};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

const SESSIONS_DIR: &str = "sessions";
const BLOBS_DIR: &str = "blobs";
const EXTENSION: &str = "jsonl";

/// 会话 ID 的最大长度
const MAX_ID_LEN: usize = 128;

/// 把会话保存为 JSONL 文件的存储
///
/// 同一会话同时只能有一个写入方，跨进程互斥由宿主保证。文件操作在阻塞线程池中执行。
#[derive(Debug)]
pub struct JsonlStore {
    root: PathBuf,
    /// 本进程已打开的会话文件，写入位置都在文件末尾
    files: Mutex<HashMap<SessionId, Arc<Mutex<File>>>>,
}

impl JsonlStore {
    /// 以 `root` 为根目录；目录在第一次写入时才创建
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            files: Mutex::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn files(&self) -> MutexGuard<'_, HashMap<SessionId, Arc<Mutex<File>>>> {
        lock(&self.files)
    }

    /// 会话文件路径；ID 只允许字母、数字、`-` 与 `_`，不会逃出根目录
    fn session_path(&self, id: &SessionId) -> Result<PathBuf, StoreError> {
        if !is_valid_id(id.as_str()) {
            return Err(StoreError::InvalidId(id.clone()));
        }
        let name = format!("{id}.{EXTENSION}");
        Ok(self.root.join(SESSIONS_DIR).join(name))
    }

    /// 会话文件的追加句柄：复用已打开的，否则打开并修复末尾残行
    async fn file(&self, id: &SessionId) -> Result<Arc<Mutex<File>>, StoreError> {
        if let Some(file) = self.files().get(id) {
            return Ok(file.clone());
        }
        let path = self.session_path(id)?;
        let session = id.clone();
        let file = blocking(move || match disk::open_for_append(&path) {
            Ok(Some(file)) => Ok(file),
            Ok(None) => Err(StoreError::Corrupt {
                id: session,
                line: 1,
                reason: "缺少完整的文件头".into(),
            }),
            Err(error) => Err(not_found_as(error, || StoreError::NotFound(session))),
        })
        .await?;
        let file = Arc::new(Mutex::new(file));
        Ok(self.files().entry(id.clone()).or_insert(file).clone())
    }
}

#[async_trait]
impl SessionStore for JsonlStore {
    async fn create(&self, header: &SessionHeader) -> Result<(), StoreError> {
        let path = self.session_path(&header.id)?;
        let line = format::encode_header(header)?;
        let dir = self.root.join(SESSIONS_DIR);
        let id = header.id.clone();
        let file = blocking(move || {
            disk::create_dir(&dir)?;
            disk::create_file(&path, &line).map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    StoreError::AlreadyExists(id)
                } else {
                    error.into()
                }
            })
        })
        .await?;
        self.files()
            .insert(header.id.clone(), Arc::new(Mutex::new(file)));
        Ok(())
    }

    async fn append(&self, id: &SessionId, entries: &[Entry]) -> Result<(), StoreError> {
        let file = self.file(id).await?;
        if entries.is_empty() {
            return Ok(());
        }
        let lines = format::encode_entries(entries)?;
        let written = blocking(move || disk::append(&mut lock(&file), &lines)).await;
        if written.is_err() {
            // 半截写入已尽力截回；丢弃句柄，下次打开时再修复末尾
            self.files().remove(id);
        }
        Ok(written?)
    }

    async fn sync(&self, id: &SessionId) -> Result<(), StoreError> {
        let file = self.file(id).await?;
        Ok(blocking(move || lock(&file).sync_data()).await?)
    }

    async fn load(&self, id: &SessionId) -> Result<StoredSession, StoreError> {
        let path = self.session_path(id)?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|error| not_found_as(error, || StoreError::NotFound(id.clone())))?;
        format::parse(id, &bytes)
    }

    async fn list(&self) -> Result<Vec<SessionHeader>, StoreError> {
        let dir = self.root.join(SESSIONS_DIR);
        let mut headers = blocking(move || read_headers(&dir)).await?;
        headers.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(headers)
    }

    async fn put_blob(&self, blob: &BlobRef, data: &[u8]) -> Result<(), StoreError> {
        let dir = self.root.join(BLOBS_DIR);
        let name = blob.as_str().to_string();
        let data = data.to_vec();
        Ok(blocking(move || {
            disk::create_dir(&dir)?;
            disk::write_blob(&dir, &name, &data)
        })
        .await?)
    }

    async fn get_blob(&self, blob: &BlobRef) -> Result<Vec<u8>, StoreError> {
        let path = self.root.join(BLOBS_DIR).join(blob.as_str());
        tokio::fs::read(&path)
            .await
            .map_err(|error| not_found_as(error, || StoreError::BlobNotFound(blob.clone())))
    }
}

/// 读取目录下全部会话的文件头；无法读取文件头的会话被跳过，目录不存在时为空
fn read_headers(dir: &Path) -> io::Result<Vec<SessionHeader>> {
    let reader = match std::fs::read_dir(dir) {
        Ok(reader) => reader,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut headers = Vec::new();
    for entry in reader {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(EXTENSION) {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let id = SessionId::new(id);
        let header = disk::read_first_line(&path)
            .ok()
            .and_then(|line| format::parse_header(&id, &line).ok());
        headers.extend(header);
    }
    Ok(headers)
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// "不存在"映射为指定的存储错误，其余保留为 IO 错误
fn not_found_as(error: io::Error, missing: impl FnOnce() -> StoreError) -> StoreError {
    if error.kind() == io::ErrorKind::NotFound {
        missing()
    } else {
        error.into()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 在阻塞线程池中执行文件操作
async fn blocking<T, E>(task: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, E>
where
    T: Send + 'static,
    E: From<io::Error> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result,
        Err(error) => Err(io::Error::other(error).into()),
    }
}
