//! shell 工具：输出整理、退出状态、尾部截断与落盘、超时及工具集装配

use crate::support::{at, call, detail, root, text_of, tool, tools, End, FakeShell, MemoryFs};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use zach_agent::SharedTool;
use zach_coding_agent::TruncationLimits;

fn bash(fs: &Arc<MemoryFs>, shell: Arc<FakeShell>) -> SharedTool {
    tool(&tools(fs).with_shell(shell).coding(), "bash")
}

/// 1 到 `n` 每个数字一行
fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{i}\n")).collect()
}

#[tokio::test]
async fn successful_commands_return_output_and_run_in_the_root() {
    let fs = MemoryFs::new();
    let shell = FakeShell::new(&["hello\n", "world\n"], End::Exit(Some(0)));
    let outcome = call(&bash(&fs, shell.clone()), json!({ "command": "echo hi" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "hello\nworld");
    assert!(!outcome.is_error());
    assert_eq!(detail(&outcome, "exit_code"), 0);
    assert_eq!(detail(&outcome, "timed_out"), false);
    assert_eq!(shell.requests(), [("echo hi".to_string(), root())]);
}

#[tokio::test]
async fn unsuccessful_exits_become_error_results() {
    let fs = MemoryFs::new();
    let failed = FakeShell::new(&["boom\n"], End::Exit(Some(2)));
    let outcome = call(&bash(&fs, failed), json!({ "command": "x" }))
        .await
        .unwrap();
    assert!(outcome.is_error());
    assert_eq!(text_of(&outcome), "boom\n\n命令退出码 2");
    assert_eq!(detail(&outcome, "exit_code"), 2);

    let killed = FakeShell::new(&[], End::Exit(None));
    let outcome = call(&bash(&fs, killed), json!({ "command": "x" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "命令被信号终止");
    assert_eq!(detail(&outcome, "exit_code"), &Value::Null);
}

#[tokio::test]
async fn silent_success_is_stated_explicitly() {
    let fs = MemoryFs::new();
    let shell = FakeShell::new(&[], End::Exit(Some(0)));
    let outcome = call(&bash(&fs, shell), json!({ "command": "true" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "（命令执行成功，无输出）");
}

#[tokio::test]
async fn terminal_control_sequences_are_cleaned() {
    let fs = MemoryFs::new();
    let shell = FakeShell::new(
        &["\u{1b}[32mok\u{1b}[0m\n", "10%\r100%\n"],
        End::Exit(Some(0)),
    );
    let outcome = call(&bash(&fs, shell), json!({ "command": "x" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "ok\n100%");
}

#[tokio::test]
async fn long_output_keeps_the_tail_and_spills_the_full_text() {
    let fs = MemoryFs::new();
    let lines = numbered(10);
    let chunks: Vec<&str> = lines.iter().map(String::as_str).collect();
    let limits = TruncationLimits {
        max_lines: 3,
        max_bytes: 1024,
    };
    let tools = tools(&fs)
        .with_limits(limits)
        .with_spill_dir(at("tmp"))
        .with_shell(FakeShell::new(&chunks, End::Exit(Some(0))));
    let outcome = call(
        &tool(&tools.coding(), "bash"),
        json!({ "command": "seq 10" }),
    )
    .await
    .unwrap();

    let path = detail(&outcome, "full_output_path").as_str().unwrap();
    assert!(path.starts_with("tmp/zach-shell-") && path.ends_with(".log"));
    assert_eq!(
        text_of(&outcome),
        format!("8\n9\n10\n\n[显示最后 3 行，共 10 行。完整输出：{path}]")
    );
    assert_eq!(fs.text(&at(path)), Some(lines.concat()));
    let truncation = detail(&outcome, "truncation");
    assert_eq!(
        (
            truncation["total_lines"].clone(),
            truncation["output_lines"].clone()
        ),
        (json!(10), json!(3))
    );
}

#[tokio::test]
async fn without_a_spill_dir_truncated_output_is_not_saved() {
    let fs = MemoryFs::new();
    let lines = numbered(5);
    let chunks: Vec<&str> = lines.iter().map(String::as_str).collect();
    let limits = TruncationLimits {
        max_lines: 2,
        max_bytes: 1024,
    };
    let tools = tools(&fs)
        .with_limits(limits)
        .with_shell(FakeShell::new(&chunks, End::Exit(Some(0))));
    let outcome = call(&tool(&tools.coding(), "bash"), json!({ "command": "x" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "4\n5\n\n[显示最后 2 行，共 5 行]");
    assert_eq!(detail(&outcome, "full_output_path"), &Value::Null);
}

/// 超时后返回已产生的输出；模型请求的超时不能超过上限
#[tokio::test(start_paused = true)]
async fn timeouts_report_partial_output_and_are_capped() {
    let fs = MemoryFs::new();
    let hang = || FakeShell::new(&["partial\n"], End::Hang);

    let outcome = call(&bash(&fs, hang()), json!({ "command": "x", "timeout": 5 }))
        .await
        .unwrap();
    assert!(outcome.is_error());
    assert_eq!(text_of(&outcome), "partial\n\n命令超时（5 秒），已终止");
    assert_eq!(detail(&outcome, "timed_out"), true);
    assert_eq!(detail(&outcome, "duration_ms"), 5000);

    let outcome = call(
        &bash(&fs, hang()),
        json!({ "command": "x", "timeout": 99999 }),
    )
    .await
    .unwrap();
    assert!(text_of(&outcome).ends_with("命令超时（600 秒），已终止"));

    let tools = tools(&fs)
        .with_command_timeout(Duration::from_secs(30))
        .with_shell(hang());
    let outcome = call(&tool(&tools.coding(), "bash"), json!({ "command": "x" }))
        .await
        .unwrap();
    assert!(text_of(&outcome).ends_with("命令超时（30 秒），已终止"));
}

#[tokio::test]
async fn spawn_failures_are_tool_errors() {
    let fs = MemoryFs::new();
    let shell = FakeShell::new(&[], End::Fail);
    let error = call(&bash(&fs, shell), json!({ "command": "x" }))
        .await
        .unwrap_err();
    assert_eq!(error.message(), "无法执行命令: 找不到程序");
}

#[test]
fn shell_tool_is_named_after_the_shell_and_only_present_when_configured() {
    let fs = MemoryFs::new();
    let names = |tools: Vec<SharedTool>| -> Vec<String> {
        tools.iter().map(|tool| tool.name().to_string()).collect()
    };
    assert_eq!(
        names(tools(&fs).coding()),
        ["read", "write", "edit", "grep", "find", "ls"]
    );

    let shell = FakeShell::new(&[], End::Exit(Some(0)));
    let with_shell = tools(&fs).with_shell(shell);
    assert_eq!(
        names(with_shell.coding()),
        ["read", "write", "edit", "grep", "find", "ls", "bash"]
    );
    assert_eq!(
        names(with_shell.read_only()),
        ["read", "grep", "find", "ls"]
    );
    let bash = tool(&with_shell.coding(), "bash");
    let description = bash.definition().description.as_deref().unwrap();
    assert!(description.contains("执行 bash 命令") && description.contains("默认超时 120 秒"));
}
