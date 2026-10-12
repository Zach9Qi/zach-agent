//! 上下文压缩：上下文接近模型窗口时，把较早的消息替换为结构化摘要
//!
//! 触发时机：运行开始前、轮次之间（超过阈值）、模型报告上下文超长后（重试一次），以及手动调用。
//! 压缩只追加一个 `compaction` 条目，原有条目全部保留。

mod plan;
mod summarizer;

pub use summarizer::{ModelSummarizer, Summarizer, SummaryRequest};

use super::entry::{CompactionEntry, CompactionReason, EntryBody, EntryId};
use super::error::SessionError;
use super::run::SessionEvent;
use super::shared::Shared;
use plan::{estimate_tokens, plan, touched_files};
use std::fmt;
use std::sync::Arc;
use zach_ai_core::LanguageModel;

/// 压缩选项
#[derive(Clone)]
pub struct CompactionOptions {
    pub(crate) auto: bool,
    pub(crate) reserve_tokens: u64,
    pub(crate) keep_recent_tokens: u64,
    pub(crate) context_window: Option<u64>,
    pub(crate) summarizer: Arc<dyn Summarizer>,
    pub(crate) read_tools: Vec<String>,
    pub(crate) write_tools: Vec<String>,
}

impl Default for CompactionOptions {
    fn default() -> Self {
        Self {
            auto: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
            context_window: None,
            summarizer: Arc::new(ModelSummarizer::new()),
            read_tools: vec!["read".into()],
            write_tools: vec!["write".into(), "edit".into()],
        }
    }
}

impl CompactionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否在超过阈值或上下文超长时自动压缩，默认开启；关闭后只能手动压缩
    pub fn with_auto(mut self, auto: bool) -> Self {
        self.auto = auto;
        self
    }

    /// 为回复与下一轮输入预留的 token：估算值超过"窗口 − 预留"即压缩，默认 16384
    pub fn with_reserve_tokens(mut self, tokens: u64) -> Self {
        self.reserve_tokens = tokens;
        self
    }

    /// 压缩时原样保留的最近消息的 token 数，默认 20000
    pub fn with_keep_recent_tokens(mut self, tokens: u64) -> Self {
        self.keep_recent_tokens = tokens;
        self
    }

    /// 上下文窗口；不设置时取模型档案的输入上限，二者都没有则不自动压缩
    pub fn with_context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// 摘要生成器，默认 [`ModelSummarizer`]（使用会话当前的模型）
    pub fn with_summarizer(mut self, summarizer: impl Summarizer + 'static) -> Self {
        self.summarizer = Arc::new(summarizer);
        self
    }

    /// 以入参 `path` 记录"读过的文件"与"改过的文件"的工具名，默认 `read` 与 `write`、`edit`
    pub fn with_file_tools(
        mut self,
        read: impl IntoIterator<Item = impl Into<String>>,
        write: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.read_tools = read.into_iter().map(Into::into).collect();
        self.write_tools = write.into_iter().map(Into::into).collect();
        self
    }

    fn window(&self, model: &dyn LanguageModel) -> Option<u64> {
        self.context_window.or_else(|| {
            let limits = &model.profile()?.limits;
            Some(limits.max_input_tokens.unwrap_or(limits.context_window))
        })
    }
}

impl fmt::Debug for CompactionOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompactionOptions")
            .field("auto", &self.auto)
            .field("reserve_tokens", &self.reserve_tokens)
            .field("keep_recent_tokens", &self.keep_recent_tokens)
            .field("context_window", &self.context_window)
            .finish_non_exhaustive()
    }
}

/// 压缩过程的事件
#[derive(Debug, Clone, PartialEq)]
pub enum CompactionEvent {
    Started {
        reason: CompactionReason,
        tokens_before: u64,
    },
    Finished {
        reason: CompactionReason,
        entry: EntryId,
    },
    Failed {
        reason: CompactionReason,
        error: String,
    },
}

impl Shared {
    /// 自动压缩开启且估算超过"窗口 − 预留"
    pub(super) fn over_threshold(&self, model: &dyn LanguageModel) -> bool {
        let options = &self.options.compaction;
        let Some(window) = options.window(model).filter(|_| options.auto) else {
            return false;
        };
        let tokens = estimate_tokens(&self.lock().log.branch());
        tokens > window.saturating_sub(options.reserve_tokens)
    }

    /// 执行一次压缩；没有可压缩的内容时返回 `Ok(None)`
    pub(super) async fn compact(
        &self,
        model: &Arc<dyn LanguageModel>,
        reason: CompactionReason,
    ) -> Result<Option<EntryId>, SessionError> {
        self.ensure_healthy()?;
        let options = &self.options.compaction;
        // 窗口较小的模型上，保留量不超过窗口的一半，否则永远无内容可压缩
        let keep = options
            .window(model.as_ref())
            .map_or(options.keep_recent_tokens, |window| {
                options.keep_recent_tokens.min(window / 2)
            });
        let (plan, tokens_before) = {
            let state = self.lock();
            let branch = state.log.branch();
            (plan(&branch, keep), estimate_tokens(&branch))
        };
        let Some(plan) = plan else {
            return Ok(None);
        };
        self.notify(CompactionEvent::Started {
            reason,
            tokens_before,
        });

        let previous = plan.previous.as_ref();
        let request = SummaryRequest {
            messages: &plan.summarized,
            previous: previous.map(|entry| entry.summary.as_str()),
            model,
            max_output_tokens: self.summary_budget(model.as_ref()),
        };
        let summary = match options.summarizer.summarize(request).await {
            Ok(summary) => summary,
            Err(error) => {
                let error = error.to_string();
                self.notify(CompactionEvent::Failed {
                    reason,
                    error: error.clone(),
                });
                return Err(SessionError::Compaction(error));
            }
        };
        let (read_files, modified_files) = touched_files(
            &plan.summarized,
            previous,
            &options.read_tools,
            &options.write_tools,
        );
        let entry = CompactionEntry {
            summary: with_file_lists(summary, &read_files, &modified_files),
            first_kept: Some(plan.first_kept),
            tokens_before,
            reason,
            read_files,
            modified_files,
        };
        let id = self.record(EntryBody::Compaction(entry)).await?;
        self.notify(CompactionEvent::Finished { reason, entry: id });
        Ok(Some(id))
    }

    /// 摘要的输出上限：预留的八成，且不超过模型的输出上限
    fn summary_budget(&self, model: &dyn LanguageModel) -> u32 {
        let budget = self.options.compaction.reserve_tokens * 4 / 5;
        let budget = model.profile().map_or(budget, |profile| {
            budget.min(profile.limits.max_output_tokens)
        });
        u32::try_from(budget.max(1)).unwrap_or(u32::MAX)
    }

    fn notify(&self, event: CompactionEvent) {
        self.send_event(SessionEvent::Compaction(event));
    }
}

/// 把文件列表附在摘要末尾，供模型在压缩后知道动过哪些文件
fn with_file_lists(mut summary: String, read: &[String], modified: &[String]) -> String {
    for (tag, files) in [("read-files", read), ("modified-files", modified)] {
        if !files.is_empty() {
            summary.push_str(&format!("\n\n<{tag}>\n{}\n</{tag}>", files.join("\n")));
        }
    }
    summary
}
