//! 命令执行抽象：shell 工具经由它运行命令，宿主可替换为沙箱、容器或远程实现

mod local;

pub use local::LocalShell;

use async_trait::async_trait;
use std::io;
use std::path::Path;

/// 一次命令执行请求
#[derive(Debug, Clone, Copy)]
pub struct ShellRequest<'a> {
    /// 命令文本，按该 shell 的语法解释
    pub command: &'a str,
    /// 工作目录（绝对路径）
    pub cwd: &'a Path,
}

/// 命令的退出状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellExit {
    /// 退出码；被信号终止等情况下没有退出码
    pub code: Option<i32>,
}

/// 输出接收方：stdout 与 stderr 按到达顺序交错送入
pub type OutputSink<'a> = dyn FnMut(&[u8]) + Send + 'a;

/// 运行命令的 shell
#[async_trait]
pub trait Shell: Send + Sync {
    /// shell 名称，用作工具名并告知模型命令语法（如 `bash`、`powershell`）
    fn name(&self) -> &str;

    /// 运行命令直到退出，输出随到随送入 `output`
    ///
    /// 返回的 future 被丢弃（超时、中止）时，实现必须终止该命令派生的整个进程树。
    async fn run(
        &self,
        request: ShellRequest<'_>,
        output: &mut OutputSink<'_>,
    ) -> io::Result<ShellExit>;
}
