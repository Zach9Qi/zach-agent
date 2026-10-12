//! 头部/尾部截断的行数、字节数上限与统计信息

use serde_json::json;
use zach_coding_agent::{truncate_head, truncate_tail, TruncatedBy, TruncationLimits};

fn limits(max_lines: usize, max_bytes: usize) -> TruncationLimits {
    TruncationLimits {
        max_lines,
        max_bytes,
    }
}

/// `n` 行文本，每行为其行号
fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("{i}\n")).collect()
}

#[test]
fn text_within_limits_is_kept_whole() {
    let result = truncate_head("a\nb\n", limits(10, 100));
    assert!(!result.is_truncated());
    assert_eq!(result.content, "a\nb");
    assert_eq!((result.total_lines, result.output_lines), (2, 2));
    assert_eq!(result.total_bytes, 4);

    let empty = truncate_tail("", limits(10, 100));
    assert_eq!((empty.content.as_str(), empty.total_lines), ("", 0));
}

#[test]
fn head_keeps_leading_lines_up_to_the_line_limit() {
    let result = truncate_head(&numbered(10), limits(3, 1000));
    assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
    assert_eq!(result.content, "1\n2\n3");
    assert_eq!((result.total_lines, result.output_lines), (10, 3));
}

#[test]
fn tail_keeps_trailing_lines_up_to_the_line_limit() {
    let result = truncate_tail(&numbered(10), limits(3, 1000));
    assert_eq!(result.truncated_by, Some(TruncatedBy::Lines));
    assert_eq!(result.content, "8\n9\n10");
    assert!(!result.partial_line);
}

/// 字节上限只截在行边界上，`\n` 分隔符也计入字节数
#[test]
fn byte_limit_never_splits_lines_and_counts_separators() {
    let result = truncate_head("aaaa\nbbbb\ncccc", limits(100, 9));
    assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!(result.content, "aaaa\nbbbb");
    assert_eq!(result.output_bytes, 9);

    let result = truncate_tail("aaaa\nbbbb\ncccc", limits(100, 8));
    assert_eq!(result.content, "cccc");
}

#[test]
fn oversized_first_line_yields_empty_head() {
    let result = truncate_head(&format!("{}\nshort", "x".repeat(50)), limits(100, 10));
    assert_eq!(result.truncated_by, Some(TruncatedBy::Bytes));
    assert_eq!((result.content.as_str(), result.output_lines), ("", 0));
}

/// 尾部截断保留超长末行的结尾片段，且不切断多字节字符
#[test]
fn oversized_last_line_keeps_its_tail_on_a_char_boundary() {
    let result = truncate_tail("开头\n一二三四五", limits(100, 7));
    assert!(result.partial_line);
    assert_eq!(result.content, "四五");
    assert_eq!(result.output_lines, 1);
}

#[test]
fn serialized_details_omit_the_content() {
    let result = truncate_head(&numbered(5), limits(2, 1000));
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "truncated_by": "lines",
            "total_lines": 5,
            "total_bytes": 10,
            "output_lines": 2,
            "output_bytes": 3,
            "partial_line": false,
        })
    );
}
