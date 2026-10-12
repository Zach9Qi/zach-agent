//! 在本机启动子进程运行命令

mod tree;

use super::{OutputSink, Shell, ShellExit, ShellRequest};
use async_trait::async_trait;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tree::TreeGuard;

/// 进程退出后，管道空闲这么久即视为输出结束
///
/// 后台派生的孙进程可能一直占着管道，不能无限等待 EOF；但退出瞬间仍可能有输出在途，
/// 每收到一块输出都会重新计时。
const DRAIN_IDLE: Duration = Duration::from_millis(100);

/// 单次读取的缓冲区大小
const READ_CHUNK: usize = 8192;

/// 以本机子进程运行命令的 shell
///
/// 命令文本作为最后一个参数传给程序；继承当前进程的环境变量。
#[derive(Debug, Clone)]
pub struct LocalShell {
    name: String,
    program: PathBuf,
    args: Vec<String>,
    prefix: Option<String>,
}

impl LocalShell {
    /// 以名称、程序与前置参数构造，例如 `("bash", "/bin/bash", ["-c"])`
    pub fn new(
        name: impl Into<String>,
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            prefix: None,
        }
    }

    /// 探测本机的 POSIX shell：Unix 上依次尝试 bash 与 sh，Windows 上查找 Git Bash
    pub fn detect() -> Option<Self> {
        if cfg!(windows) {
            return git_bash().map(|program| Self::new("bash", program, ["-c"]));
        }
        ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|program| Self::new("bash", program, ["-c"]))
            .or_else(|| {
                Path::new("/bin/sh")
                    .is_file()
                    .then(|| Self::new("sh", "/bin/sh", ["-c"]))
            })
    }

    /// Windows PowerShell：优先 PowerShell 7（`pwsh`），输出统一为 UTF-8
    pub fn powershell() -> Self {
        let program = find_on_path("pwsh.exe").unwrap_or_else(|| "powershell.exe".into());
        let mut shell = Self::new(
            "powershell",
            program,
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
            ],
        );
        shell.prefix = Some("[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;".into());
        shell
    }

    fn command(&self, request: ShellRequest<'_>) -> Command {
        let text = match &self.prefix {
            Some(prefix) => format!("{prefix}{}", request.command),
            None => request.command.to_string(),
        };
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .arg(text)
            .current_dir(request.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tree::isolate(&mut command);
        command
    }
}

#[async_trait]
impl Shell for LocalShell {
    fn name(&self) -> &str {
        &self.name
    }

    async fn run(
        &self,
        request: ShellRequest<'_>,
        output: &mut OutputSink<'_>,
    ) -> io::Result<ShellExit> {
        let mut child = self.command(request).spawn()?;
        // 先于 child 析构：被丢弃时先清理整棵进程树，再由 kill_on_drop 兜底
        let mut guard = TreeGuard::attach(&child);
        let status = pump(&mut child, output).await?;
        guard.disarm();
        Ok(ShellExit {
            code: status.code(),
        })
    }
}

/// 交错读取 stdout 与 stderr，直到进程退出且管道排空（或空闲超时）
async fn pump(child: &mut Child, output: &mut OutputSink<'_>) -> io::Result<ExitStatus> {
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut out_buf = vec![0; READ_CHUNK];
    let mut err_buf = vec![0; READ_CHUNK];
    let mut status = None;
    loop {
        if let (Some(status), None, None) = (status, &stdout, &stderr) {
            return Ok(status);
        }
        tokio::select! {
            read = read_some(&mut stdout, &mut out_buf) => match read {
                Some(n) => output(&out_buf[..n]),
                None => stdout = None,
            },
            read = read_some(&mut stderr, &mut err_buf) => match read {
                Some(n) => output(&err_buf[..n]),
                None => stderr = None,
            },
            exited = child.wait(), if status.is_none() => status = Some(exited?),
            _ = tokio::time::sleep(DRAIN_IDLE), if status.is_some() => {
                return Ok(status.expect("已由分支条件保证"));
            }
        }
    }
}

/// 读取一块输出；EOF 或读取失败返回 `None`，已关闭的管道永远挂起
async fn read_some<R: AsyncRead + Unpin>(reader: &mut Option<R>, buf: &mut [u8]) -> Option<usize> {
    match reader {
        Some(reader) => match reader.read(buf).await {
            Ok(0) | Err(_) => None,
            Ok(n) => Some(n),
        },
        None => std::future::pending().await,
    }
}

/// 在常见安装位置与 PATH 中查找 Git Bash，排除 System32 下的 WSL 启动器
fn git_bash() -> Option<PathBuf> {
    let installed = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .flat_map(|base| {
            [
                base.join("Git").join("bin").join("bash.exe"),
                base.join("Programs")
                    .join("Git")
                    .join("bin")
                    .join("bash.exe"),
            ]
        })
        .find(|path| path.is_file());
    installed.or_else(|| {
        find_on_path("bash.exe").filter(|path| {
            !path
                .to_string_lossy()
                .to_lowercase()
                .contains("\\windows\\system32\\")
        })
    })
}

/// 在 PATH 中查找可执行文件
fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}
