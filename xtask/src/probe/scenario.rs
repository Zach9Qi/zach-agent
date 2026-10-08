//! 场景沿用标准调用参数，加载本地文件简写与共享 Schema 后执行断言。

mod files;
mod schema;

use super::{config::Mode, expect::Expectation, ProbeResult};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use zach_ai_core::{CallOptions, ToolChoice};

pub(super) struct Scenario {
    pub(super) request: CallOptions,
    pub(super) expect: Vec<Expectation>,
    pub(super) timeout_secs: u64,
    pub(super) agent: Option<AgentSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    request: Value,
    expect: Vec<Expectation>,
    #[serde(default)]
    schema_file: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    agent: Option<AgentSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentSettings {
    #[serde(default)]
    pub(super) tools: Vec<String>,
    #[serde(default = "default_max_steps")]
    pub(super) max_steps: usize,
    /// 省略时保留请求中的工具策略；显式配置时在后续轮次应用。
    #[serde(default)]
    pub(super) next_tool_choice: Option<ToolChoice>,
}

fn default_timeout() -> u64 {
    120
}
fn default_max_steps() -> usize {
    4
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            tools: vec![],
            max_steps: default_max_steps(),
            next_tool_choice: None,
        }
    }
}

impl Scenario {
    pub(super) fn load(mode: Mode, path: &Path) -> ProbeResult<Self> {
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("读取场景文件失败: {}", e.kind()))?;
        Self::parse(mode, &source, path, |path| {
            std::fs::read(path).map_err(|e| e.kind().to_string())
        })
    }

    pub(super) fn parse(
        mode: Mode,
        source: &str,
        path: &Path,
        mut read: impl FnMut(&Path) -> ProbeResult<Vec<u8>>,
    ) -> ProbeResult<Self> {
        let mut ignored = false;
        let mut deserializer =
            serde_json::Deserializer::from_str(source.trim_start_matches('\u{feff}'));
        let mut draft: Draft = serde_ignored::deserialize(&mut deserializer, |_| ignored = true)
            // 解析错误可能带入 headers 或正文中的凭据，不回显原始值。
            .map_err(|e| {
                format!(
                    "场景 JSON 格式无效（第 {} 行，第 {} 列），请检查字段和标准类型",
                    e.line(),
                    e.column()
                )
            })?;
        deserializer
            .end()
            .map_err(|_| "场景 JSON 尾部存在多余内容")?;
        if ignored {
            return Err("场景包含标准类型不识别的字段，请检查拼写".into());
        }
        // 仅在内存中补齐待加载字段，先验证标准请求和断言，再读取附件。
        let files = files::prepare(&mut draft.request)?;
        if let Some(schema_file) = &draft.schema_file {
            schema::prepare(&mut draft.request, schema_file)?;
        }
        let request = serde_ignored::deserialize(&draft.request, |_| ignored = true)
            .map_err(|_| "标准 request 格式无效：请检查字段类型；文件必须提供 data 或 path")?;
        if ignored {
            return Err("场景包含标准类型不识别的字段，请检查拼写".into());
        }
        let mut scenario = Self {
            request,
            expect: draft.expect,
            timeout_secs: draft.timeout_secs,
            agent: draft.agent,
        };
        scenario.validate(mode)?;
        if let Some(schema_file) = &draft.schema_file {
            schema::bind(&mut scenario.request, schema_file, path, &mut read)?;
        }
        files::bind(&mut scenario.request, &files, path, read)?;
        Ok(scenario)
    }

    fn validate(&self, mode: Mode) -> ProbeResult<()> {
        if !(1..=3600).contains(&self.timeout_secs) {
            return Err("timeout_secs 必须是 1..=3600 内的整数".into());
        }
        if self.request.prompt.is_empty() || self.expect.is_empty() {
            return Err("场景必须包含非空 request.prompt.messages 和 expect".into());
        }
        if self.request.max_output_tokens == Some(0) {
            return Err("request.max_output_tokens 必须大于零".into());
        }
        match (&self.agent, mode) {
            (settings, Mode::Agent) => {
                let defaults = AgentSettings::default();
                let agent = settings.as_ref().unwrap_or(&defaults);
                if !(1..=100).contains(&agent.max_steps) {
                    return Err("agent.max_steps 必须是 1..=100 内的整数".into());
                }
                if agent.tools.iter().any(|name| name != "add") || agent.tools.len() > 1 {
                    return Err("agent.tools 当前仅支持 add，且不能重复注册".into());
                }
                if self.request.tools.is_some() {
                    return Err(
                        "agent 模式通过 agent.tools 注册可执行工具，不能设置 request.tools".into(),
                    );
                }
                for choice in [&self.request.tool_choice, &agent.next_tool_choice] {
                    match choice {
                        Some(ToolChoice::Tool { tool_name })
                            if !agent.tools.contains(tool_name) =>
                        {
                            return Err("工具选择指向未注册的 agent.tools".into())
                        }
                        Some(ToolChoice::Required) if agent.tools.is_empty() => {
                            return Err("required 工具策略需要注册 agent.tools".into())
                        }
                        _ => {}
                    }
                }
            }
            (Some(_), _) => return Err("agent 配置仅适用于 agent 模式".into()),
            _ => {}
        }
        for expectation in &self.expect {
            expectation.validate(mode)?;
        }
        Ok(())
    }

    pub(super) fn max_steps(&self, mode: Mode) -> usize {
        if mode == Mode::Agent {
            self.agent
                .as_ref()
                .map_or(default_max_steps(), |agent| agent.max_steps)
        } else {
            1
        }
    }
}
