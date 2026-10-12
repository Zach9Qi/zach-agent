//! 文件系统抽象：工具的所有文件读写都经由它完成
//!
//! 宿主可以替换为沙箱、容器、远程或内存实现；工具逻辑因此与真实磁盘解耦，
//! 测试也可以使用纯 async 的内存替身。

mod local;

pub use local::LocalFileSystem;

use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};

/// 路径的元数据
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileInfo {
    /// 是否为目录
    pub is_dir: bool,
    /// 文件字节数（目录无意义）
    pub len: u64,
}

/// 目录项
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEntry {
    /// 绝对路径
    pub path: PathBuf,
    /// 是否为目录
    pub is_dir: bool,
}

/// 工具使用的文件系统
///
/// 所有路径都是 [`crate::Workspace::resolve`] 解析后的绝对路径。
/// 路径不存在时应返回 [`io::ErrorKind::NotFound`]，工具据此给出友好提示。
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// 读取文件的全部字节
    async fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// 写入文件，已存在则覆盖；父目录须已存在
    async fn write(&self, path: &Path, data: &[u8]) -> io::Result<()>;

    /// 递归创建目录，已存在时不报错
    async fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// 路径的元数据，跟随符号链接
    async fn metadata(&self, path: &Path) -> io::Result<FileInfo>;

    /// 目录的直接子项，包含隐藏项，顺序不限
    async fn read_dir(&self, path: &Path) -> io::Result<Vec<FsEntry>>;

    /// 递归列出目录下的文件与子目录（不含目录自身），顺序不限
    ///
    /// 应遵守 `.gitignore` 等忽略规则、跳过 `.git` 目录，但保留其余隐藏文件。
    async fn walk(&self, path: &Path) -> io::Result<Vec<FsEntry>>;
}
