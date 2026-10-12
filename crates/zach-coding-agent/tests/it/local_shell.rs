//! 本机 shell：输出收集、退出码、工作目录与被丢弃时终止整棵进程树
//!
//! 需要本机安装 bash（Windows 上为 Git Bash）。这里验证的是真实子进程的行为，
//! 只有进程树清理一例需要真实等待。

use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use zach_coding_agent::{LocalShell, Shell, ShellExit, ShellRequest};

fn bash() -> LocalShell {
    LocalShell::detect().expect("需要本机安装 bash（Windows 上为 Git Bash）")
}

/// 运行命令并收集全部输出
async fn run(shell: &LocalShell, command: &str, cwd: &Path) -> (String, ShellExit) {
    let mut collected = Vec::new();
    let mut sink = |chunk: &[u8]| collected.extend_from_slice(chunk);
    let exit = shell
        .run(ShellRequest { command, cwd }, &mut sink)
        .await
        .unwrap();
    (String::from_utf8_lossy(&collected).into_owned(), exit)
}

#[tokio::test]
async fn stdout_and_stderr_are_collected_with_the_exit_code() {
    let dir = TempDir::new().unwrap();
    let (output, exit) = run(&bash(), "echo out; echo err 1>&2; exit 3", dir.path()).await;
    assert!(output.contains("out\n"), "{output}");
    assert!(output.contains("err\n"), "{output}");
    assert_eq!(exit, ShellExit { code: Some(3) });
}

#[tokio::test]
async fn commands_run_in_the_requested_directory() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("marker.txt"), "here").unwrap();
    let (output, exit) = run(&bash(), "cat marker.txt", dir.path()).await;
    assert_eq!((output.as_str(), exit.code), ("here", Some(0)));
}

/// 防止只杀掉直接子进程、后台孙进程继续运行
#[tokio::test]
async fn dropping_a_running_command_kills_its_whole_process_tree() {
    let dir = TempDir::new().unwrap();
    let command = "(sleep 1; echo late > late.txt) & sleep 30";
    let shell = bash();
    let mut sink = |_: &[u8]| {};
    let request = ShellRequest {
        command,
        cwd: dir.path(),
    };
    let result = tokio::time::timeout(Duration::from_millis(300), shell.run(request, &mut sink));
    assert!(result.await.is_err(), "命令应仍在运行");

    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(!dir.path().join("late.txt").exists(), "后台进程未被终止");
}
