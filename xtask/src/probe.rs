//! 统一真实模型联调入口，仅由开发者显式执行，不纳入自动化门禁。

mod agent;
mod args;
mod config;
mod expect;
mod factory;
mod local_env;
mod observe;
mod output;
mod runner;
mod scenario;
mod trace;

#[cfg(test)]
mod tests;

use args::Args;
use config::Config;
use local_env::LocalEnv;
use output::Reporter;
use scenario::Scenario;
use std::{error::Error, sync::Arc};

type ProbeResult<T> = Result<T, String>;

const HELP: &str = "\
用法: cargo xtask probe --scenario <场景.json> [--mode <模式>] [连接选项]

  --scenario <文件>      场景文件：标准 request、schema_file、expect
  --mode <模式>          generate、stream（默认）、agent；同一场景可切换执行方式
  --protocol <协议>       当前支持 openai-responses（默认）
  --model <模型 ID>       覆盖 PROBE_MODEL
  --base-url <根地址>     覆盖 PROBE_BASE_URL
  --api-key-env <变量名>  指定存放密钥的环境变量；默认 PROBE_API_KEY
  --help, -h             显示帮助，不读取凭据或访问网络

输出为逐行 JSON：配置、通用请求、事件、结果与耗时。凭据会脱敏。
自动读取仓库根目录 .env.local；同名变量优先使用进程环境，不修改进程环境。
输入、推理、原始事件、超时、Agent 工具和预期由场景配置，执行模式由 --mode 选择。
本地附件直接在消息的文件块中填写 path，加载后转换为标准 data。
内置场景见 scenarios/probe，格式见 docs/probe.md。
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
    let scenario = Scenario::load(config.mode, &config.scenario_path)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("创建异步运行时失败: {e}"))?;
    runtime.block_on(async {
        let model = factory::create(
            &config,
            std::time::Duration::from_secs(scenario.timeout_secs),
        )?;
        tokio::select! {
            result = runner::run(&config, &scenario, model, reporter) => result,
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|e| format!("监听 Ctrl+C 失败: {e}"))?;
                Err("联调已取消".into())
            }
        }
    })
}
