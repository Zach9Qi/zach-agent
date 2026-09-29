//! 提交前门禁：依次执行格式检查、Clippy 与测试，任一失败即停止
//!
//! 本地与 CI 共用同一入口，避免"本地通过、CI 挂掉"的分歧。

use std::error::Error;
use std::process::Command;

/// 检查步骤：名称与传给 `cargo` 的参数
const STEPS: &[(&str, &[&str])] = &[
    ("格式检查", &["fmt", "--all", "--", "--check"]),
    (
        "Clippy",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    ),
    ("测试", &["test", "--workspace"]),
];

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    for (name, args) in STEPS {
        eprintln!("==> {name}: cargo {}", args.join(" "));
        let status = Command::new(&cargo).args(*args).status()?;
        if !status.success() {
            return Err(format!("{name}未通过（cargo {}）", args.join(" ")).into());
        }
    }
    eprintln!("==> 全部检查通过");
    Ok(())
}
