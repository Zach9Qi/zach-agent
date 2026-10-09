//! 联调配置：协议别名、按厂商分组的环境变量、默认场景与套件展开。

use super::{args::Args, ProbeResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 内置场景目录；默认场景与 `.env.local` 一样固定从仓库解析，不依赖当前工作目录。
fn bundled_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 位于仓库子目录")
        .join("scenarios")
        .join("probe")
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(super) enum Protocol {
    #[serde(rename = "openai-responses")]
    OpenAiResponses,
    #[serde(rename = "openai-chat")]
    OpenAiChat,
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
}

impl Protocol {
    fn parse(value: &str) -> ProbeResult<Self> {
        match value {
            "openai-responses" | "responses" => Ok(Self::OpenAiResponses),
            "openai-chat" | "chat" => Ok(Self::OpenAiChat),
            "anthropic-messages" | "anthropic" => Ok(Self::AnthropicMessages),
            _ => Err(
                "未知协议；可用 openai-responses(responses)、openai-chat(chat)、anthropic-messages(anthropic)"
                    .into(),
            ),
        }
    }

    /// 短别名，同时是内置工具场景的文件名前缀。
    fn short(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "responses",
            Self::OpenAiChat => "chat",
            Self::AnthropicMessages => "anthropic",
        }
    }

    /// 环境变量分组：同一厂商的协议共用连接配置。
    fn env_group(self) -> &'static str {
        match self {
            Self::OpenAiResponses | Self::OpenAiChat => "OPENAI",
            Self::AnthropicMessages => "ANTHROPIC",
        }
    }

    fn default_url(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "https://api.openai.com/v1",
            Self::OpenAiChat => "https://api.openai.com/v1",
            Self::AnthropicMessages => "https://api.anthropic.com",
        }
    }

    /// 适配器自动追加的端点后缀；根地址不应再包含它。
    fn endpoint_suffix(self) -> &'static str {
        match self {
            Self::OpenAiResponses => "/responses",
            Self::OpenAiChat => "/chat/completions",
            Self::AnthropicMessages => "/messages",
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

    /// 内置套件中该模式使用的场景：单轮模式共用 mixed.json，Agent 使用协议专属工具场景。
    fn bundled_scenario(self, protocol: Protocol) -> PathBuf {
        let name = match self {
            Self::Generate | Self::Stream => "mixed.json".to_owned(),
            Self::Agent => format!("{}-tools.json", protocol.short()),
        };
        bundled_dir().join(name)
    }
}

/// 一次独立运行的完整配置；省略模式时一次命令展开为多次运行，连接配置相同。
pub(super) struct Config {
    pub(super) protocol: Protocol,
    pub(super) mode: Mode,
    pub(super) model: String,
    pub(super) base_url: String,
    pub(super) api_key: String,
    pub(super) scenario_path: PathBuf,
}

impl Config {
    /// 按命令行与环境变量展开要执行的运行列表，顺序固定为 generate、stream、agent。
    ///
    /// - 指定模式：只运行该模式；场景省略时使用该模式的内置场景。
    /// - 省略模式且省略场景：运行内置套件（三种模式）。
    /// - 省略模式但指定场景：自定义场景默认以 stream 运行。
    pub(super) fn resolve(
        args: Args,
        env: impl Fn(&str) -> Option<String>,
    ) -> ProbeResult<Vec<Self>> {
        let protocol = Protocol::parse(args.get("protocol").unwrap_or("openai-responses"))?;
        let mode = args.get("mode").map(Mode::parse).transpose()?;
        let scenario = args.get("scenario").map(PathBuf::from);
        let runs: Vec<(Mode, PathBuf)> = match (mode, scenario) {
            (Some(mode), Some(path)) => vec![(mode, path)],
            (Some(mode), None) => vec![(mode, mode.bundled_scenario(protocol))],
            (None, Some(path)) => vec![(Mode::Stream, path)],
            (None, None) => [Mode::Generate, Mode::Stream, Mode::Agent]
                .into_iter()
                .map(|mode| (mode, mode.bundled_scenario(protocol)))
                .collect(),
        };
        // 查找顺序：--选项 → PROBE_<厂商>_X → PROBE_X；显式留空视为缺失，不回退到另一组凭据。
        let grouped = |variable: &str| {
            env(&format!("PROBE_{}_{variable}", protocol.env_group()))
                .or_else(|| env(&format!("PROBE_{variable}")))
        };
        let setting = |name: &str, variable: &str| {
            args.get(name)
                .map(str::to_owned)
                .or_else(|| grouped(variable))
        };
        let model = setting("model", "MODEL")
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "请通过 --model、PROBE_{}_MODEL 或 PROBE_MODEL 指定模型 ID",
                    protocol.env_group()
                )
            })?;
        let base_url =
            setting("base-url", "BASE_URL").unwrap_or_else(|| protocol.default_url().into());
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
        if base_url.ends_with(protocol.endpoint_suffix()) {
            return Err("根地址不应包含协议端点后缀，适配器会自动追加".into());
        }
        let api_key = match args.get("api-key-env") {
            Some(variable) => env(variable),
            None => grouped("API_KEY"),
        }
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "缺少 API Key：设置 PROBE_{}_API_KEY 或 PROBE_API_KEY，或使用 --api-key-env 指定变量名",
                protocol.env_group()
            )
        })?;
        Ok(runs
            .into_iter()
            .map(|(mode, scenario_path)| Self {
                protocol,
                mode,
                model: model.clone(),
                base_url: base_url.clone(),
                api_key: api_key.clone(),
                scenario_path,
            })
            .collect())
    }
}
