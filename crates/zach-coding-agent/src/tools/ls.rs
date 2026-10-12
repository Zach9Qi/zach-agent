//! ls 工具：列出目录的直接子项

use super::ToolEnv;
use crate::truncate::truncate_head;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};

/// 默认最多列出的条目数
const DEFAULT_LIMIT: usize = 500;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LsInput {
    /// 目录路径，默认为工作区根目录
    #[serde(default)]
    path: Option<String>,
    /// 最多列出的条目数，默认 500
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) struct LsTool {
    env: ToolEnv,
}

impl LsTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        Self { env }
    }
}

#[async_trait]
impl TypedTool for LsTool {
    type Input = LsInput;

    fn name(&self) -> &str {
        "ls"
    }

    fn description(&self) -> &str {
        "列出目录的直接子项（含隐藏文件），按名称排序，目录以 / 结尾。\
         递归查找文件请用 find。"
    }

    fn needs_approval(&self, input: &LsInput) -> bool {
        self.env
            .needs_approval(input.path.as_deref().unwrap_or("."), false)
    }

    async fn call(&self, input: LsInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let path = self
            .env
            .resolve(input.path.as_deref().unwrap_or("."), false)?;
        let display = self.env.display(&path);
        let entries = self
            .env
            .fs
            .read_dir(&path)
            .await
            .map_err(|error| self.env.io_error(error, &path))?;

        let mut names: Vec<String> = entries
            .iter()
            .map(|entry| {
                let name = entry
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if entry.is_dir {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        names.sort_by_cached_key(|name| (name.to_lowercase(), name.clone()));
        let total = names.len();
        if total == 0 {
            return Ok(ToolOutcome::text(format!("（{display} 是空目录）"))
                .with_details(json!({ "path": display, "entries": 0, "shown": 0 })));
        }

        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let truncation = truncate_head(&names[..total.min(limit)].join("\n"), self.env.limits);
        let shown = truncation.output_lines;
        let mut text = truncation.content;
        if shown < total {
            text.push_str(&format!(
                "\n\n[共 {total} 项，只显示前 {shown} 项。可用 limit 调整，或用 find 按模式查找]"
            ));
        }
        Ok(ToolOutcome::text(text).with_details(json!({
            "path": display,
            "entries": total,
            "shown": shown,
        })))
    }
}
