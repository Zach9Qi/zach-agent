//! 基于本地磁盘的文件系统实现

use super::{FileInfo, FileSystem, FsEntry};
use async_trait::async_trait;
use ignore::WalkBuilder;
use std::io;
use std::path::{Path, PathBuf};

/// 直接读写本地磁盘；递归遍历遵守 `.gitignore`（不要求位于 git 仓库内）
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFileSystem;

#[async_trait]
impl FileSystem for LocalFileSystem {
    async fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        tokio::fs::read(path).await
    }

    async fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        tokio::fs::write(path, data).await
    }

    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn metadata(&self, path: &Path) -> io::Result<FileInfo> {
        let metadata = tokio::fs::metadata(path).await?;
        Ok(FileInfo {
            is_dir: metadata.is_dir(),
            len: metadata.len(),
        })
    }

    async fn read_dir(&self, path: &Path) -> io::Result<Vec<FsEntry>> {
        let mut reader = tokio::fs::read_dir(path).await?;
        let mut entries = Vec::new();
        while let Some(entry) = reader.next_entry().await? {
            // 跟随符号链接判断类型，失效链接按文件处理
            let is_dir = tokio::fs::metadata(entry.path())
                .await
                .is_ok_and(|metadata| metadata.is_dir());
            entries.push(FsEntry {
                path: entry.path(),
                is_dir,
            });
        }
        Ok(entries)
    }

    async fn walk(&self, path: &Path) -> io::Result<Vec<FsEntry>> {
        // 先确认根目录可访问，避免把"不存在"吞成空结果
        tokio::fs::metadata(path).await?;
        let root = path.to_path_buf();
        tokio::task::spawn_blocking(move || walk_blocking(root))
            .await
            .map_err(io::Error::other)
    }
}

/// 同步遍历；子目录不可读等错误直接跳过
fn walk_blocking(root: PathBuf) -> Vec<FsEntry> {
    WalkBuilder::new(&root)
        .hidden(false)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .flatten()
        .filter(|entry| entry.depth() > 0)
        .map(|entry| FsEntry {
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
            path: entry.into_path(),
        })
        .collect()
}
