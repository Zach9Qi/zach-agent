//! 编码工具集：按工作区、文件系统与截断上限组装可交给 `zach-agent` 的工具

mod locks;
mod read;
mod text;
mod write;

use crate::fs::{FileSystem, LocalFileSystem};
use crate::truncate::TruncationLimits;
use crate::workspace::{OutsideAccess, Workspace};
use locks::FileLocks;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zach_agent::{typed_tool, SharedTool, ToolError};

/// 编码工具集的构造器
///
/// 同一个构造器产出的写类工具共享按文件的互斥锁：并行执行时，对同一文件的修改会排队，
/// 不同文件仍然并发。
#[derive(Clone)]
pub struct CodingTools {
    env: ToolEnv,
}

impl CodingTools {
    /// 以工作区与文件系统构造
    pub fn new(workspace: Workspace, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            env: ToolEnv {
                workspace: Arc::new(workspace),
                fs,
                locks: Arc::new(FileLocks::default()),
                limits: TruncationLimits::default(),
            },
        }
    }

    /// 以本地磁盘构造
    pub fn local(workspace: Workspace) -> Self {
        Self::new(workspace, Arc::new(LocalFileSystem))
    }

    /// 设置输出截断上限，默认 2000 行或 50KB
    pub fn with_limits(mut self, limits: TruncationLimits) -> Self {
        self.env.limits = limits;
        self
    }

    /// 只读工具：read
    pub fn read_only(&self) -> Vec<SharedTool> {
        vec![self.read()]
    }

    /// 编码工具：read、write
    pub fn coding(&self) -> Vec<SharedTool> {
        vec![self.read(), self.write()]
    }

    fn read(&self) -> SharedTool {
        typed_tool(read::ReadTool::new(self.env.clone()))
    }

    fn write(&self) -> SharedTool {
        typed_tool(write::WriteTool::new(self.env.clone()))
    }
}

/// 工具共享的运行环境
#[derive(Clone)]
pub(crate) struct ToolEnv {
    pub(crate) workspace: Arc<Workspace>,
    pub(crate) fs: Arc<dyn FileSystem>,
    pub(crate) locks: Arc<FileLocks>,
    pub(crate) limits: TruncationLimits,
}

impl ToolEnv {
    /// 解析模型给出的路径并执行越界策略：拒绝时返回错误
    pub(crate) fn resolve(&self, raw: &str, write: bool) -> Result<PathBuf, ToolError> {
        let path = self.workspace.resolve(raw);
        match self.workspace.access(&path, write) {
            OutsideAccess::Deny => Err(ToolError::failed(format!(
                "{} 位于工作区之外，不允许{}",
                path.display(),
                if write { "写入" } else { "读取" }
            ))),
            OutsideAccess::Allow | OutsideAccess::Ask => Ok(path),
        }
    }

    /// 该路径是否需要人工审批
    pub(crate) fn needs_approval(&self, raw: &str, write: bool) -> bool {
        let path = self.workspace.resolve(raw);
        self.workspace.access(&path, write) == OutsideAccess::Ask
    }

    /// 给模型看的路径
    pub(crate) fn display(&self, path: &Path) -> String {
        self.workspace.display(path)
    }

    /// 把文件系统错误转成带路径的工具错误
    pub(crate) fn io_error(&self, error: io::Error, path: &Path) -> ToolError {
        let path = self.display(path);
        match error.kind() {
            io::ErrorKind::NotFound => ToolError::failed(format!("路径不存在: {path}")),
            io::ErrorKind::PermissionDenied => ToolError::failed(format!("没有权限访问: {path}")),
            _ => ToolError::failed(format!("访问 {path} 失败: {error}")),
        }
    }
}
