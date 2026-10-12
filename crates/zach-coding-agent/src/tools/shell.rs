//! shell 工具：在工作区根目录执行命令，返回清洗、截断后的输出末尾
//!
//! 输出只保留末尾给模型（结论通常在最后）；被截断时完整输出写入落盘目录，
//! 路径同时告知模型与宿主。超时或运行被中止时，命令的整棵进程树都会被终止。

mod output;
mod sanitize;

use super::ToolEnv;
use crate::shell::{Shell, ShellExit, ShellRequest};
use crate::truncate::truncate_tail;
use async_trait::async_trait;
use output::{OutputBuffer, SPILL_CAP};
use sanitize::sanitize;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::hash::{BuildHasher, RandomState};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};
use zach_ai_core::ToolResultOutput;

/// 模型可指定的最长超时
const MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// 进度上报的最小间隔
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ShellInput {
    /// 要执行的命令
    command: String,
    /// 超时秒数，最长 600 秒；不填使用默认值
    #[serde(default)]
    timeout: Option<u64>,
}

/// shell 工具的配置
#[derive(Clone)]
pub(crate) struct ShellConfig {
    pub(crate) shell: Arc<dyn Shell>,
    pub(crate) spill_dir: Option<PathBuf>,
    pub(crate) timeout: Duration,
}

pub(crate) struct ShellTool {
    env: ToolEnv,
    config: ShellConfig,
    description: String,
}

impl ShellTool {
    pub(crate) fn new(env: ToolEnv, config: ShellConfig) -> Self {
        let spill = if config.spill_dir.is_some() {
            "；被截断时完整输出会保存到文件，可用 read 查看"
        } else {
            ""
        };
        let description = format!(
            "在工作区根目录执行 {} 命令，返回 stdout 与 stderr 的合并输出。\
             输出只保留最后 {} 行或 {}KB（先到者为准）{spill}。\
             默认超时 {} 秒，可用 timeout 调整（最长 600 秒）。\
             不要运行需要交互输入或不会自行结束的命令（如开发服务器、watch 模式），\
             这类命令请放到后台运行。",
            config.shell.name(),
            env.limits.max_lines,
            env.limits.max_bytes / 1024,
            config.timeout.as_secs(),
        );
        Self {
            env,
            config,
            description,
        }
    }

    /// 运行命令；`Ok(None)` 表示超时
    async fn run(
        &self,
        command: &str,
        timeout: Duration,
        buffer: &mut OutputBuffer,
        ctx: &ToolContext,
    ) -> Result<Option<ShellExit>, ToolError> {
        let limits = self.env.limits;
        let mut last_report = Instant::now();
        let mut sink = |chunk: &[u8]| {
            buffer.push(chunk);
            let now = Instant::now();
            if now.duration_since(last_report) >= PROGRESS_INTERVAL {
                last_report = now;
                let preview = truncate_tail(&sanitize(&buffer.tail_text()), limits).content;
                ctx.report_progress(ToolResultOutput::text(preview));
            }
        };
        let request = ShellRequest {
            command,
            cwd: self.env.workspace.root(),
        };
        let run = self.config.shell.run(request, &mut sink);
        match tokio::time::timeout(timeout, run).await {
            Ok(Ok(exit)) => Ok(Some(exit)),
            Ok(Err(error)) => Err(ToolError::failed(format!("无法执行命令: {error}"))),
            Err(_) => Ok(None),
        }
    }

    /// 把完整输出写入落盘目录，返回给模型看的路径
    async fn spill(&self, buffer: &OutputBuffer) -> Result<Option<(String, bool)>, ToolError> {
        let Some(dir) = &self.config.spill_dir else {
            return Ok(None);
        };
        let (head, complete) = buffer.head();
        let text = sanitize(&String::from_utf8_lossy(head));
        let random = RandomState::new().hash_one(std::time::SystemTime::now());
        let path = dir.join(format!("zach-shell-{random:016x}.log"));
        let fs = &self.env.fs;
        fs.create_dir_all(dir)
            .await
            .map_err(|error| self.env.io_error(error, dir))?;
        fs.write(&path, text.as_bytes())
            .await
            .map_err(|error| self.env.io_error(error, &path))?;
        Ok(Some((self.env.display(&path), complete)))
    }
}

#[async_trait]
impl TypedTool for ShellTool {
    type Input = ShellInput;

    fn name(&self) -> &str {
        self.config.shell.name()
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn call(&self, input: ShellInput, ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let timeout = input.timeout.map_or(self.config.timeout, |secs| {
            Duration::from_secs(secs).min(MAX_TIMEOUT.max(self.config.timeout))
        });
        let head_cap = if self.config.spill_dir.is_some() {
            SPILL_CAP
        } else {
            0
        };
        let mut buffer = OutputBuffer::new(self.env.limits.max_bytes * 2, head_cap);
        let started = Instant::now();
        let exit = self.run(&input.command, timeout, &mut buffer, &ctx).await?;
        let duration = started.elapsed();

        let truncation = truncate_tail(&sanitize(&buffer.tail_text()), self.env.limits);
        let truncated = truncation.is_truncated() || buffer.dropped();
        let spilled = if truncated {
            self.spill(&buffer).await?
        } else {
            None
        };

        let mut text = truncation.content.clone();
        if truncated {
            let shown = if truncation.partial_line {
                format!(
                    "最后一行过长，只显示其末尾 {} 字节",
                    truncation.output_bytes
                )
            } else {
                format!(
                    "显示最后 {} 行，共 {} 行",
                    truncation.output_lines,
                    buffer.total_lines()
                )
            };
            let full = match &spilled {
                Some((path, true)) => format!("。完整输出：{path}"),
                Some((path, false)) => format!("。输出前 {}MB：{path}", SPILL_CAP >> 20),
                None => String::new(),
            };
            text.push_str(&format!("\n\n[{shown}{full}]"));
        }

        let status = match exit {
            None => Some(format!("命令超时（{} 秒），已终止", timeout.as_secs())),
            Some(ShellExit { code: Some(0) }) => None,
            Some(ShellExit { code: Some(code) }) => Some(format!("命令退出码 {code}")),
            Some(ShellExit { code: None }) => Some("命令被信号终止".to_string()),
        };
        let mut details = json!({
            "exit_code": exit.and_then(|exit| exit.code),
            "timed_out": exit.is_none(),
            "duration_ms": u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
        });
        if truncated {
            let mut stats = json!(truncation);
            stats["total_lines"] = Value::from(buffer.total_lines());
            stats["total_bytes"] = Value::from(buffer.total_bytes());
            details["truncation"] = stats;
        }
        if let Some((path, _)) = &spilled {
            details["full_output_path"] = Value::from(path.as_str());
        }

        let outcome = match status {
            Some(status) => {
                let body = if text.is_empty() {
                    status
                } else {
                    format!("{text}\n\n{status}")
                };
                ToolOutcome::error(body)
            }
            None if text.is_empty() => ToolOutcome::text("（命令执行成功，无输出）"),
            None => ToolOutcome::text(text),
        };
        Ok(outcome.with_details(details))
    }
}
