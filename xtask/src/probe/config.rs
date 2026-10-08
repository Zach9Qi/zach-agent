//! 联调配置：协议选择、命令行优先级及可注入的环境变量读取。

use super::{args::Args, ProbeResult};
use serde::Serialize;
use std::time::Duration;
use zach_ai_core::CallOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(super) enum Protocol {
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
}

impl Protocol {
    fn parse(value: &str) -> ProbeResult<Self> {
        match value {
            "openai-responses" => Ok(Self::OpenAiResponses),
            "openai-chat" | "anthropic-messages" => {
                Err("该协议尚未实现；当前可用协议为 openai-responses".into())
            }
            _ => Err("未知协议；当前可用协议为 openai-responses".into()),
        }
    }

    fn defaults(self) -> (&'static str, &'static str) {
        match self {
            Self::OpenAiResponses => ("OPENAI", "https://api.openai.com/v1"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    Generate,
    Stream,
    Agent,
}

pub(super) struct Config {
    pub(super) protocol: Protocol,
    pub(super) mode: Mode,
    pub(super) model: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) prompt: String,
    pub(super) timeout: Duration,
    pub(super) max_steps: usize,
    pub(super) options: CallOptions,
}

impl Config {
    pub(super) fn resolve(args: Args, env: impl Fn(&str) -> Option<String>) -> ProbeResult<Self> {
        let protocol = Protocol::parse(args.get("protocol").unwrap_or("openai-responses"))?;
        let mode = match args.get("mode").unwrap_or("stream") {
            "generate" => Mode::Generate,
            "stream" => Mode::Stream,
            "agent" => Mode::Agent,
            _ => return Err("未知模式；可用模式为 generate、stream、agent".into()),
        };
        if mode != Mode::Agent && args.get("max-steps").is_some() {
            return Err("--max-steps 仅适用于 agent 模式".into());
        }
        let timeout = Duration::from_secs(number(&args, "timeout-secs", 120, 1, 3600)?);
        let max_steps = number(&args, "max-steps", 4, 2, 100)? as usize;
        let max_output_tokens =
            number(&args, "max-output-tokens", 4096, 1, u32::MAX as u64)? as u32;
        let (prefix, default_url) = protocol.defaults();
        let setting = |name: &str, variable: &str| {
            args.get(name)
                .map(str::to_owned)
                .or_else(|| env(&format!("PROBE_{variable}")))
                .or_else(|| env(&format!("{prefix}_{variable}")))
        };
        let model = setting("model", "MODEL")
            .filter(|s| !s.trim().is_empty())
            .ok_or("请通过 --model、PROBE_MODEL 或协议对应的 MODEL 环境变量指定模型 ID")?;
        let base_url = setting("base-url", "BASE_URL").unwrap_or_else(|| default_url.into());
        let parsed = reqwest::Url::parse(&base_url).map_err(|_| "根地址不是有效 URL")?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err("根地址须为 HTTP(S) URL，不能包含用户名、密码、查询或片段".into());
        }
        let base_url = base_url.trim_end_matches('/').to_owned();
        if protocol == Protocol::OpenAiResponses && base_url.ends_with("/responses") {
            return Err("根地址不应包含 /responses 后缀，适配器会自动追加".into());
        }
        let api_key = if let Some(variable) = args.get("api-key-env") {
            env(variable)
        } else {
            env("PROBE_API_KEY").or_else(|| env(&format!("{prefix}_API_KEY")))
        }
        .filter(|s| !s.trim().is_empty())
        .ok_or(
            "缺少 API Key：设置 PROBE_API_KEY、协议对应的 API_KEY，或使用 --api-key-env 指定变量名",
        )?;
        let default_prompt = if mode == Mode::Agent {
            "请调用 add 工具计算 40 + 2，并根据工具结果给出答案。"
        } else {
            "请只回复：联调成功"
        };
        Ok(Self {
            protocol,
            mode,
            model,
            base_url,
            api_key,
            prompt: args.get("prompt").unwrap_or(default_prompt).into(),
            timeout,
            max_steps,
            options: CallOptions {
                max_output_tokens: Some(max_output_tokens),
                include_raw_chunks: args.raw,
                ..Default::default()
            },
        })
    }
}

fn number(args: &Args, name: &str, default: u64, min: u64, max: u64) -> ProbeResult<u64> {
    let Some(raw) = args.get(name) else {
        return Ok(default);
    };
    raw.parse::<u64>()
        .ok()
        .filter(|v| (min..=max).contains(v))
        .ok_or_else(|| format!("--{name} 必须是 {min}..={max} 内的整数"))
}
