//! 系统提示词：身份说明、工作规则、宿主附加内容、项目指令文件与工作区信息
//!
//! 开头的身份说明不加标签，其余各段以同名 XML 标签包裹，便于模型区分来源。
//! 渲染结果交给 `Agent` 的系统提示词；接入会话时，提示词的变化会记录在配置条目中。

mod context;

pub use context::{ContextFile, ContextFiles};

use crate::workspace::Workspace;
use std::collections::HashSet;
use zach_agent::SharedTool;

/// 默认的身份说明
const DEFAULT_PREAMBLE: &str = "你是一名资深的编码助手，在用户的工作区中工作：阅读文件、执行命令、\
     修改代码、编写新文件，帮助用户完成软件开发任务。";

/// 编码智能体的系统提示词构造器
#[derive(Debug, Clone)]
pub struct SystemPrompt {
    workspace: Workspace,
    preamble: String,
    tool_names: Vec<String>,
    rules: Vec<String>,
    addendum: Option<String>,
    context_files: Vec<ContextFile>,
}

impl SystemPrompt {
    pub fn new(workspace: &Workspace) -> Self {
        Self {
            workspace: workspace.clone(),
            preamble: DEFAULT_PREAMBLE.to_string(),
            tool_names: Vec::new(),
            rules: Vec::new(),
            addendum: None,
            context_files: Vec::new(),
        }
    }

    /// 替换开头的身份说明
    pub fn with_preamble(mut self, preamble: impl Into<String>) -> Self {
        self.preamble = preamble.into();
        self
    }

    /// 交给 Agent 的工具，据此给出相应的使用规则
    pub fn with_tools(mut self, tools: &[SharedTool]) -> Self {
        self.tool_names = tools.iter().map(|tool| tool.name().to_string()).collect();
        self
    }

    /// 追加一条工作规则
    pub fn with_rule(mut self, rule: impl Into<String>) -> Self {
        self.rules.push(rule.into());
        self
    }

    /// 宿主附加的内容，位于项目指令之前
    pub fn with_addendum(mut self, addendum: impl Into<String>) -> Self {
        self.addendum = Some(addendum.into());
        self
    }

    /// 项目指令文件，按给定顺序列出（通常来自 [`ContextFiles::discover`]）
    pub fn with_context_files(mut self, files: Vec<ContextFile>) -> Self {
        self.context_files = files;
        self
    }

    /// 渲染为完整的系统提示词
    pub fn render(&self) -> String {
        let mut sections = vec![self.preamble.trim().to_string()];
        sections.push(tagged("rules", &self.render_rules()));
        if let Some(addendum) = self.addendum.as_deref().map(str::trim) {
            if !addendum.is_empty() {
                sections.push(tagged("addendum", addendum));
            }
        }
        if !self.context_files.is_empty() {
            sections.push(tagged("project_context", &self.render_context()));
        }
        let workspace = format!(
            "工作区根目录：{}\n相对路径以工作区根目录为基准。",
            self.workspace.root().display()
        );
        sections.push(tagged("workspace", &workspace));
        sections.join("\n\n")
    }

    /// 工具相关的规则在前，其后是宿主的规则与通用规则；去掉空白与重复的条目
    fn render_rules(&self) -> String {
        let has = |name: &str| self.tool_names.iter().any(|tool| tool == name);
        let mut rules = Vec::new();
        if has("read") && (has("edit") || has("write")) {
            rules.push("修改已有文件前先用 read 查看要改动的部分，不要凭猜测修改".to_string());
        }
        if has("edit") && has("write") {
            rules
                .push("修改已有文件用 edit 做精确替换，新建文件或整体重写时才用 write".to_string());
        }
        rules.extend(self.rules.iter().cloned());
        rules.push("回答简洁直接".to_string());
        rules.push("提到文件时写出清晰的路径".to_string());

        let mut seen = HashSet::new();
        rules
            .iter()
            .map(|rule| rule.trim())
            .filter(|rule| !rule.is_empty() && seen.insert(*rule))
            .map(|rule| format!("- {rule}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_context(&self) -> String {
        let mut parts = vec![
            "以下是项目中的指令与约定，必须遵守。越靠后的文件来自越靠近工作区的目录，\
             内容冲突时以靠后的为准。"
                .to_string(),
        ];
        for file in &self.context_files {
            parts.push(format!(
                "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
                self.workspace.display(&file.path),
                file.content
            ));
        }
        parts.join("\n\n")
    }
}

fn tagged(tag: &str, content: &str) -> String {
    format!("<{tag}>\n{content}\n</{tag}>")
}
