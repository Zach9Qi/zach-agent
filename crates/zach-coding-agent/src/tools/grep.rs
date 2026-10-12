//! grep 工具：按正则或字面量在文件内容中搜索，遵守 .gitignore，可附带上下文行

mod matches;

use super::pattern::{relative_to, PathMatcher};
use super::text::is_binary;
use super::ToolEnv;
use crate::truncate::truncate_head;
use async_trait::async_trait;
use matches::Matches;
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};

/// 默认最多返回的匹配数
const DEFAULT_LIMIT: usize = 100;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct GrepInput {
    /// 搜索模式，默认按正则表达式解释（Rust regex 语法）
    pattern: String,
    /// 搜索的文件或目录，默认为工作区根目录
    #[serde(default)]
    path: Option<String>,
    /// 只搜索匹配该 glob 的文件，如 `*.rs`、`src/**/*.ts`
    #[serde(default)]
    glob: Option<String>,
    /// 忽略大小写，默认 false
    #[serde(default)]
    ignore_case: Option<bool>,
    /// 把 pattern 当作字面量而非正则，默认 false
    #[serde(default)]
    literal: Option<bool>,
    /// 每处匹配前后附带的上下文行数，默认 0
    #[serde(default)]
    context: Option<usize>,
    /// 最多返回的匹配数，默认 100
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) struct GrepTool {
    env: ToolEnv,
}

impl GrepTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        Self { env }
    }
}

#[async_trait]
impl TypedTool for GrepTool {
    type Input = GrepInput;

    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "在文件内容中搜索正则表达式（或 literal=true 时的字面量），遵守 .gitignore，跳过二进制文件。\
         结果每行形如 `路径:行号: 内容`，上下文行形如 `路径-行号- 内容`，不相邻的片段以 -- 分隔。\
         默认最多 100 条匹配，超过 500 字符的行会被截断。"
    }

    fn needs_approval(&self, input: &GrepInput) -> bool {
        self.env
            .needs_approval(input.path.as_deref().unwrap_or("."), false)
    }

    async fn call(&self, input: GrepInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let source = if input.literal.unwrap_or(false) {
            regex::escape(&input.pattern)
        } else {
            input.pattern.clone()
        };
        let regex = RegexBuilder::new(&source)
            .case_insensitive(input.ignore_case.unwrap_or(false))
            .build()
            .map_err(|error| ToolError::invalid_input(format!("正则表达式无效: {error}")))?;
        let filter = input
            .glob
            .as_deref()
            .map(PathMatcher::new)
            .transpose()
            .map_err(|error| ToolError::invalid_input(format!("glob 模式无效: {error}")))?;

        let target = self
            .env
            .resolve(input.path.as_deref().unwrap_or("."), false)?;
        let fs = &self.env.fs;
        let info = fs
            .metadata(&target)
            .await
            .map_err(|error| self.env.io_error(error, &target))?;
        let files = if info.is_dir {
            let entries = fs
                .walk(&target)
                .await
                .map_err(|error| self.env.io_error(error, &target))?;
            let mut files: Vec<_> = entries
                .into_iter()
                .filter(|entry| !entry.is_dir)
                .filter(|entry| {
                    filter
                        .as_ref()
                        .is_none_or(|filter| filter.matches(&relative_to(&entry.path, &target)))
                })
                .map(|entry| entry.path)
                .collect();
            files.sort();
            files
        } else {
            vec![target.clone()]
        };

        let limit = input.limit.unwrap_or(DEFAULT_LIMIT).max(1);
        let mut matches = Matches::new(limit, input.context.unwrap_or(0));
        for file in &files {
            if matches.is_full() {
                break;
            }
            // 读取失败（权限、竞态删除）的文件直接跳过，不影响整体搜索
            let Ok(bytes) = fs.read(file).await else {
                continue;
            };
            if !is_binary(&bytes) {
                let text = String::from_utf8_lossy(&bytes);
                matches.scan(&self.env.display(file), &text, &regex);
            }
        }

        if matches.count() == 0 {
            return Ok(ToolOutcome::text(format!(
                "在 {} 中没有找到匹配 {} 的内容",
                self.env.display(&target),
                input.pattern
            ))
            .with_details(json!({ "matches": 0, "files": 0 })));
        }
        let truncation = truncate_head(&matches.lines().join("\n"), self.env.limits);
        let mut notes = Vec::new();
        if matches.is_full() {
            notes.push(format!(
                "已达到 {limit} 条匹配的上限，可用 limit 调整或细化模式"
            ));
        }
        if truncation.is_truncated() {
            notes.push(format!(
                "输出超出上限，只显示前 {} 行",
                truncation.output_lines
            ));
        }
        if matches.clipped() {
            notes.push("部分行过长已截断，可用 read 查看完整内容".to_string());
        }
        let mut text = truncation.content.clone();
        if !notes.is_empty() {
            text.push_str(&format!("\n\n[{}]", notes.join("；")));
        }
        let mut details = json!({
            "matches": matches.count(),
            "files": matches.files(),
            "limit_reached": matches.is_full(),
        });
        if truncation.is_truncated() {
            details["truncation"] = json!(truncation);
        }
        Ok(ToolOutcome::text(text).with_details(details))
    }
}
