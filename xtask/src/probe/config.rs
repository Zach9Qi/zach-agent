//! 联调配置：协议选择、命令行优先级及可注入的环境变量读取。

use super::{args::Args, ProbeResult};
use serde::Serialize;
use std::path::PathBuf;

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

    fn default_url(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "https://api.openai.com/v1",
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

impl Mode {
    fn parse(value: &str) -> ProbeResult<Self> {
        match value {
            "generate" => Ok(Self::Generate),
            "stream" => Ok(Self::Stream),
            "agent" => Ok(Self::Agent),
            _ => Err("--mode 必须为 generate、stream 或 agent".into()),
        }
    }
}

pub(super) struct Config {
    pub(super) protocol: Protocol,
    pub(super) mode: Mode,
    pub(super) model: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) scenario_path: PathBuf,
}

impl Config {
    pub(super) fn resolve(args: Args, env: impl Fn(&str) -> Option<String>) -> ProbeResult<Self> {
        let protocol = Protocol::parse(args.get("protocol").unwrap_or("openai-responses"))?;
        let mode = Mode::parse(args.get("mode").unwrap_or("stream"))?;
        let scenario_path = args
            .get("scenario")
            .ok_or("请通过 --scenario 指定场景 JSON 文件")?
            .into();
        let setting = |name: &str, variable: &str| {
            args.get(name).map(str::to_owned).or_else(|| env(variable))
        };
        let model = setting("model", "PROBE_MODEL")
            .filter(|s| !s.trim().is_empty())
            .ok_or("请通过 --model 或 PROBE_MODEL 指定模型 ID")?;
        let base_url =
            setting("base-url", "PROBE_BASE_URL").unwrap_or_else(|| protocol.default_url().into());
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
        let api_key = env(args.get("api-key-env").unwrap_or("PROBE_API_KEY"))
            .filter(|s| !s.trim().is_empty())
            .ok_or("缺少 API Key：设置 PROBE_API_KEY，或使用 --api-key-env 指定变量名")?;
        Ok(Self {
            protocol,
            mode,
            model,
            base_url,
            api_key,
            scenario_path,
        })
    }
}
