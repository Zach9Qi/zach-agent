//! 输出截断：按行数与字节数上限截取文本的头部或尾部
//!
//! 读文件、搜索这类"从头往后看"的输出截取头部，命令输出这类"结论在最后"的截取尾部。
//! 截断结果附带完整的统计信息，工具据此给模型生成"如何继续"的提示，并写入结果详情。

use serde::Serialize;

/// 截断上限，行数与字节数先触及者生效
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncationLimits {
    /// 最多保留的行数
    pub max_lines: usize,
    /// 最多保留的字节数（UTF-8）
    pub max_bytes: usize,
}

impl Default for TruncationLimits {
    /// 2000 行或 50KB
    fn default() -> Self {
        Self {
            max_lines: 2000,
            max_bytes: 50 * 1024,
        }
    }
}

/// 触发截断的上限
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncatedBy {
    /// 行数上限
    Lines,
    /// 字节数上限
    Bytes,
}

/// 截断结果；序列化时只输出统计信息，不含正文
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Truncation {
    /// 保留下来的文本，行间以 `\n` 连接，末尾不带换行
    #[serde(skip)]
    pub content: String,
    /// 触发截断的上限；未截断时为 `None`
    pub truncated_by: Option<TruncatedBy>,
    /// 原文总行数
    pub total_lines: usize,
    /// 原文总字节数
    pub total_bytes: usize,
    /// 保留的行数
    pub output_lines: usize,
    /// 保留的字节数
    pub output_bytes: usize,
    /// 尾部截断时，保留的唯一一行只是原行的末尾片段
    pub partial_line: bool,
}

impl Truncation {
    /// 是否发生了截断
    pub fn is_truncated(&self) -> bool {
        self.truncated_by.is_some()
    }
}

/// 保留开头的完整行，绝不截半行
///
/// 第一行本身就超过字节上限时，结果为空（`output_lines == 0`），由调用方提示改用其他方式读取。
pub fn truncate_head(text: &str, limits: TruncationLimits) -> Truncation {
    let lines = split_lines(text);
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0;
    let mut truncated_by = None;
    for line in &lines {
        if let Some(by) = exceeds(kept.len(), bytes, line, limits) {
            truncated_by = Some(by);
            break;
        }
        bytes += line.len() + usize::from(!kept.is_empty());
        kept.push(line);
    }
    finish(text, &lines, kept, truncated_by, false)
}

/// 保留末尾的完整行
///
/// 最后一行本身就超过字节上限时，保留该行在字符边界上的末尾片段，并标记 `partial_line`。
pub fn truncate_tail(text: &str, limits: TruncationLimits) -> Truncation {
    let lines = split_lines(text);
    let mut kept: Vec<&str> = Vec::new();
    let mut bytes = 0;
    let mut truncated_by = None;
    for line in lines.iter().rev() {
        if let Some(by) = exceeds(kept.len(), bytes, line, limits) {
            truncated_by = Some(by);
            break;
        }
        bytes += line.len() + usize::from(!kept.is_empty());
        kept.push(line);
    }
    let mut partial = false;
    if kept.is_empty() && truncated_by == Some(TruncatedBy::Bytes) {
        if let Some(last) = lines.last() {
            kept.push(tail_slice(last, limits.max_bytes));
            partial = true;
        }
    }
    kept.reverse();
    finish(text, &lines, kept, truncated_by, partial)
}

/// 再保留一行是否会超出上限
fn exceeds(kept: usize, bytes: usize, line: &str, limits: TruncationLimits) -> Option<TruncatedBy> {
    if kept >= limits.max_lines {
        return Some(TruncatedBy::Lines);
    }
    let separator = usize::from(kept > 0);
    (bytes + separator + line.len() > limits.max_bytes).then_some(TruncatedBy::Bytes)
}

fn finish(
    text: &str,
    lines: &[&str],
    kept: Vec<&str>,
    truncated_by: Option<TruncatedBy>,
    partial_line: bool,
) -> Truncation {
    let content = kept.join("\n");
    Truncation {
        truncated_by,
        total_lines: lines.len(),
        total_bytes: text.len(),
        output_lines: kept.len(),
        output_bytes: content.len(),
        partial_line,
        content,
    }
}

/// 按 `\n` 切分；末尾的单个换行不计为额外的空行，空文本为零行
fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

/// 取不超过 `max_bytes` 字节的末尾片段，起点落在字符边界上
fn tail_slice(line: &str, max_bytes: usize) -> &str {
    let mut start = line.len().saturating_sub(max_bytes);
    while !line.is_char_boundary(start) {
        start += 1;
    }
    &line[start..]
}
