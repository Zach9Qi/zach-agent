//! 统一真实模型联调入口，仅由开发者显式执行，不纳入自动化门禁。

mod agent;
mod args;
mod config;
mod factory;
mod local_env;
mod observe;
mod output;
mod runner;

#[cfg(test)]
mod tests;

use args::Args;
use config::Config;
use local_env::LocalEnv;
use output::Reporter;
use std::{error::Error, sync::Arc};

type ProbeResult<T> = Result<T, String>;

const HELP: &str = "\
用法: cargo xtask probe [选项]

  --protocol <协议>       当前支持 openai-responses（默认）
  --mode <模式>           generate | stream | agent，默认 stream
  --model <模型 ID>       覆盖 PROBE_MODEL；Responses 兼容 OPENAI_MODEL
  --base-url <根地址>     覆盖 PROBE_BASE_URL；Responses 兼容 OPENAI_BASE_URL
  --api-key-env <变量名>  指定存放密钥的环境变量；默认 PROBE_API_KEY，
                         未设置时 Responses 回退到 OPENAI_API_KEY
  --prompt <文本>        覆盖内置联调提示
  --timeout-secs <秒>    整次联调超时，默认 120，范围 1..=3600
  --max-steps <轮数>     Agent 最大模型调用次数，默认 4，范围 2..=100
  --max-output-tokens <数> 每次请求的输出上限，默认 4096
  --raw                  在流式和 Agent 模式额外输出原始协议事件
  --help, -h             显示帮助，不读取凭据或访问网络

输出为逐行 JSON：配置、通用请求、事件、结果与耗时。凭据会脱敏。
自动读取仓库根目录 .env.local；同名变量优先使用进程环境，不修改进程环境。
Agent 模式注册 add 工具，验证“调用工具 → 回传结果 → 最终回复”。
真实请求可能产生 API 费用；只有显式运行本命令才会发起调用。";

pub(crate) fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(args) = Args::parse(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let local_env = LocalEnv::load()?;
    let config = Config::resolve(args, |name| {
        local_env.get(name, |name| std::env::var(name).ok())
    })?;
    let reporter = Arc::new(Reporter::new(std::io::stdout(), config.api_key.clone()));
    let result = execute(config, reporter.clone());
    if let Err(error) = result {
        reporter.emit("probe_error", &serde_json::json!({"message": error}));
        return Err(reporter.redact(&error).into());
    }
    Ok(())
}

fn execute(config: Config, reporter: Arc<Reporter>) -> ProbeResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("创建异步运行时失败: {e}"))?;
    runtime.block_on(async {
        let model = factory::create(&config)?;
        tokio::select! {
            result = runner::run(&config, model, reporter) => result,
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|e| format!("监听 Ctrl+C 失败: {e}"))?;
                Err("联调已取消".into())
            }
        }
    })
}
