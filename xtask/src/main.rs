//! 项目开发任务入口，用法：`cargo xtask <命令>`

mod sync_models;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("sync-models") => sync_models::run(),
        _ => {
            eprintln!(
                "用法: cargo xtask <命令>\n\n命令:\n  sync-models  从 models.dev 同步内置模型档案"
            );
            return ExitCode::FAILURE;
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误: {err}");
            ExitCode::FAILURE
        }
    }
}
