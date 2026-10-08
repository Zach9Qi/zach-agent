//! 项目开发任务入口，用法：`cargo xtask <命令>`

mod check;
mod probe;
mod probe_schemas;
mod sync_models;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("check") => check::run(),
        Some("sync-models") => sync_models::run(),
        Some("probe") => probe::run(&args[1..]),
        Some("probe-schemas") => probe_schemas::run(&args[1..]),
        Some("--help" | "-h") => {
            println!(
                "用法: cargo xtask <check|sync-models|probe|probe-schemas>\n联调帮助: cargo xtask probe --help"
            );
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!(
                "用法: cargo xtask <命令>\n\n命令:\n  \
                 check        依次执行格式检查、Clippy 与测试\n  \
                 sync-models  从 models.dev 同步内置模型档案\n  \
                 probe        按协议和模式联调真实模型（probe --help 查看配置）\n  \
                 probe-schemas 从 Rust 结果类型生成可复用的联调 JSON Schema"
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
