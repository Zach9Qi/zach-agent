//! find 工具：按 glob 模式递归查找文件与目录

use super::pattern::{relative_to, PathMatcher};
use super::ToolEnv;
use crate::truncate::truncate_head;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};

/// 默认最多返回的结果数
const DEFAULT_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FindInput {
    /// glob 模式，如 `*.rs`、`src/**/*.ts`、`**/test_*`
    pattern: String,
    /// 搜索的目录，默认为工作区根目录
    #[serde(default)]
    path: Option<String>,
    /// 最多返回的结果数，默认 1000
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) struct FindTool {
    env: ToolEnv,
}

impl FindTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        Self { env }
    }
}

#[async_trait]
impl TypedTool for FindTool {
    type Input = FindInput;

    fn name(&self) -> &str {
        "find"
    }

    fn description(&self) -> &str {
        "按 glob 模式递归查找文件与目录，遵守 .gitignore 并跳过 .git。\
         不含 / 的模式匹配文件名（如 *.rs）；含 / 的模式匹配相对搜索目录的路径，\
         可出现在任意深度（如 src/*.rs），以 / 开头则锚定在搜索目录下。\
         结果为排序后的路径，目录以 / 结尾。"
    }

    fn needs_approval(&self, input: &FindInput) -> bool {
        self.env
            .needs_approval(input.path.as_deref().unwrap_or("."), false)
    }

    async fn call(&self, input: FindInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let matcher = PathMatcher::new(&input.pattern)
            .map_err(|error| ToolError::invalid_input(format!("glob 模式无效: {error}")))?;
        let dir = self
            .env
            .resolve(input.path.as_deref().unwrap_or("."), false)?;
        let entries = self
            .env
            .fs
            .walk(&dir)
            .await
            .map_err(|error| self.env.io_error(error, &dir))?;

        let mut found: Vec<String> = entries
            .iter()
            .filter(|entry| matcher.matches(&relative_to(&entry.path, &dir)))
            .map(|entry| {
                let display = self.env.display(&entry.path);
                if entry.is_dir {
                    format!("{display}/")
                } else {
                    display
                }
            })
            .collect();
        found.sort();
        let total = found.len();
        if total == 0 {
            return Ok(ToolOutcome::text(format!(
                "在 {} 下没有匹配 {} 的路径",
                self.env.display(&dir),
                input.pattern
            ))
            .with_details(json!({ "matches": 0, "shown": 0 })));
        }

        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let truncation = truncate_head(&found[..total.min(limit)].join("\n"), self.env.limits);
        let shown = truncation.output_lines;
        let mut text = truncation.content;
        if shown < total {
            text.push_str(&format!(
                "\n\n[共 {total} 个结果，只显示前 {shown} 个。可缩小搜索范围或用 limit 调整]"
            ));
        }
        Ok(ToolOutcome::text(text).with_details(json!({ "matches": total, "shown": shown })))
    }
}
