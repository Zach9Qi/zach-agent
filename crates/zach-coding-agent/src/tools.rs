//! 编码工具集：按工作区、文件系统、shell 与截断上限组装可交给 `zach-agent` 的工具

mod edit;
mod find;
mod grep;
mod locks;
mod ls;
mod pattern;
mod read;
mod shell;
mod text;
mod write;

use crate::fs::{FileSystem, LocalFileSystem};
use crate::shell::{LocalShell, Shell};
use crate::truncate::TruncationLimits;
use crate::workspace::{OutsideAccess, Workspace};
use locks::FileLocks;
use shell::{ShellConfig, ShellTool};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zach_agent::{typed_tool, SharedTool, ToolError};

/// 命令的默认超时
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// 编码工具集的构造器
///
/// 同一个构造器产出的写类工具共享按文件的互斥锁：并行执行时，对同一文件的修改会排队，
/// 不同文件仍然并发。
///
/// 文件类工具按 [`Workspace`] 的越界策略决定是否需要审批；shell 工具无法得知命令会触及
/// 哪些路径，不做路径审批，命令级的放行与审批请通过 `zach_agent::AgentHooks::before_tool_call`
/// 或审批处理器实现。
#[derive(Clone)]
pub struct CodingTools {
    env: ToolEnv,
    shell: Option<Arc<dyn Shell>>,
    spill_dir: Option<PathBuf>,
    command_timeout: Duration,
}

impl CodingTools {
    /// 以工作区与文件系统构造，不带 shell
    pub fn new(workspace: Workspace, fs: Arc<dyn FileSystem>) -> Self {
        Self {
            env: ToolEnv {
                workspace: Arc::new(workspace),
                fs,
                locks: Arc::new(FileLocks::default()),
                limits: TruncationLimits::default(),
            },
            shell: None,
            spill_dir: None,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
        }
    }

    /// 以本地磁盘与本机 shell（[`LocalShell::detect`]）构造，命令的完整输出落盘到系统临时目录
    pub fn local(workspace: Workspace) -> Self {
        let mut tools = Self::new(workspace, Arc::new(LocalFileSystem));
        tools.shell = LocalShell::detect().map(|shell| Arc::new(shell) as Arc<dyn Shell>);
        tools.spill_dir = Some(std::env::temp_dir());
        tools
    }

    /// 设置输出截断上限，默认 2000 行或 50KB
    pub fn with_limits(mut self, limits: TruncationLimits) -> Self {
        self.env.limits = limits;
        self
    }

    /// 设置运行命令的 shell
    pub fn with_shell(mut self, shell: Arc<dyn Shell>) -> Self {
        self.shell = Some(shell);
        self
    }

    /// 设置命令输出被截断时完整输出的落盘目录（经由文件系统写入）；不设置则不落盘
    pub fn with_spill_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.spill_dir = Some(dir.into());
        self
    }

    /// 设置命令的默认超时，默认 120 秒
    pub fn with_command_timeout(mut self, timeout: Duration) -> Self {
        self.command_timeout = timeout;
        self
    }

    /// 只读工具：read、grep、find、ls
    pub fn read_only(&self) -> Vec<SharedTool> {
        vec![self.read(), self.grep(), self.find(), self.ls()]
    }

    /// 编码工具：read、write、edit、grep、find、ls，以及配置了 shell 时的命令工具（以 shell 名称命名）
    pub fn coding(&self) -> Vec<SharedTool> {
        let mut tools = vec![
            self.read(),
            self.write(),
            self.edit(),
            self.grep(),
            self.find(),
            self.ls(),
        ];
        tools.extend(self.shell());
        tools
    }

    fn read(&self) -> SharedTool {
        typed_tool(read::ReadTool::new(self.env.clone()))
    }

    fn write(&self) -> SharedTool {
        typed_tool(write::WriteTool::new(self.env.clone()))
    }

    fn edit(&self) -> SharedTool {
        typed_tool(edit::EditTool::new(self.env.clone()))
    }

    fn grep(&self) -> SharedTool {
        typed_tool(grep::GrepTool::new(self.env.clone()))
    }

    fn find(&self) -> SharedTool {
        typed_tool(find::FindTool::new(self.env.clone()))
    }

    fn ls(&self) -> SharedTool {
        typed_tool(ls::LsTool::new(self.env.clone()))
    }

    fn shell(&self) -> Option<SharedTool> {
        let config = ShellConfig {
            shell: self.shell.clone()?,
            spill_dir: self.spill_dir.clone(),
            timeout: self.command_timeout,
        };
        Some(typed_tool(ShellTool::new(self.env.clone(), config)))
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
