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
use std::{error::Error, pin::pin, sync::Arc};

type ProbeResult<T> = Result<T, String>;

const HELP: &str = "\
用法: cargo xtask probe [协议] [模式] [--scenario <场景.json>] [连接选项]

  协议                   responses（默认）、chat、anthropic；也接受 --protocol 及完整名称
  模式                   generate、stream、agent；省略时依次运行内置套件的三种模式
  --scenario <文件>      自定义场景（标准 request、schema_file、expect）；省略模式时默认 stream
  --model <模型 ID>       覆盖 PROBE_<厂商>_MODEL / PROBE_MODEL
  --base-url <根地址>     覆盖 PROBE_<厂商>_BASE_URL / PROBE_BASE_URL
  --api-key-env <变量名>  指定存放密钥的环境变量；默认 PROBE_<厂商>_API_KEY / PROBE_API_KEY
  --help, -h             显示帮助，不读取凭据或访问网络

示例:
  cargo xtask probe anthropic                 # 全套：generate/stream 用 mixed.json，agent 用 anthropic-tools.json
  cargo xtask probe chat agent                # 只跑 openai-chat 的 Agent 场景
  cargo xtask probe responses stream --scenario my.json

厂商分组：responses/chat 读取 PROBE_OPENAI_*，anthropic 读取 PROBE_ANTHROPIC_*，缺失时回退 PROBE_*。
输出为逐行 JSON：配置、通用请求、事件、结果与耗时。凭据会脱敏。
自动读取仓库根目录 .env.local；同名变量优先使用进程环境，不修改进程环境。
输入、推理、原始事件、超时、Agent 工具和预期由场景配置，协议与执行模式由命令行选择。
本地附件直接在消息的文件块中填写 path，加载后转换为标准 data。
内置场景见 scenarios/probe，格式见 docs/probe.md。
真实请求可能产生 API 费用；只有显式运行本命令才会发起调用。";

pub(crate) fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    let Some(args) = Args::parse(args)? else {
        println!("{HELP}");
        return Ok(());
    };
    let local_env = LocalEnv::load()?;
    let configs = Config::resolve(args, |name| {
        local_env.get(name, |name| std::env::var(name).ok())
    })?;
    let reporter = Arc::new(Reporter::new(std::io::stdout(), configs[0].api_key.clone()));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("创建异步运行时失败: {e}"))?;
    let total = configs.len();
    let mut failures = Vec::new();
    let mut summary = Vec::new();
    runtime.block_on(async {
        let mut ctrl_c = pin!(tokio::signal::ctrl_c());
        for config in &configs {
            let mut cancelled = false;
            let result = tokio::select! {
                result = execute(config, reporter.clone()) => result,
                signal = &mut ctrl_c => {
                    cancelled = true;
                    Err(match signal {
                        Ok(()) => "联调已取消".to_owned(),
                        Err(e) => format!("监听 Ctrl+C 失败: {e}"),
                    })
                }
            };
            if let Err(error) = &result {
                reporter.emit(
                    "probe_error",
                    &serde_json::json!({
                        "mode": config.mode, "scenario": config.scenario_path, "message": error,
                    }),
                );
                failures.push(format!(
                    "{:?} {}: {error}",
                    config.mode,
                    config.scenario_path.display()
                ));
            }
            summary.push(serde_json::json!({
                "mode": config.mode, "scenario": config.scenario_path, "success": result.is_ok(),
            }));
            if cancelled {
                break;
            }
        }
    });
    if total > 1 {
        reporter.emit(
            "probe_summary",
            &serde_json::json!({
                "total": total, "finished": summary.len(),
                "failed": failures.len(), "runs": summary,
            }),
        );
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(reporter.redact(&failures.join("\n")).into())
    }
}

async fn execute(config: &Config, reporter: Arc<Reporter>) -> ProbeResult<()> {
    let scenario = Scenario::load(config.mode, &config.scenario_path)?;
    let model = factory::create(
        config,
        std::time::Duration::from_secs(scenario.timeout_secs),
    )?;
    runner::run(config, &scenario, model, reporter).await
}
