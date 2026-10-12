//! 把一组替换应用到原文：精确匹配优先，失败再宽松匹配；全部对照原文定位，不允许重叠

use super::fuzzy::{occurrences, FuzzyMatch, FuzzyText};
use std::fmt;
use std::ops::Range;

/// 一处替换
#[derive(Debug, Clone)]
pub(crate) struct Replacement<'a> {
    pub(crate) old_text: &'a str,
    pub(crate) new_text: &'a str,
}

/// 应用结果
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Applied {
    /// 替换后的全文
    pub(crate) content: String,
    /// 经宽松匹配定位的替换数
    pub(crate) fuzzy: usize,
}

/// 无法应用的原因；`index` 为替换在入参中的序号
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ApplyError {
    EmptyOldText { index: usize },
    NotFound { index: usize },
    Ambiguous { index: usize, count: usize },
    Overlap { first: usize, second: usize },
    Unchanged,
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyOldText { index } => write!(f, "edits[{index}].old_text 不能为空"),
            Self::NotFound { index } => write!(
                f,
                "找不到 edits[{index}].old_text。old_text 必须与文件内容完全一致\
                 （包括缩进、空白与换行），请先用 read 确认原文"
            ),
            Self::Ambiguous { index, count } => write!(
                f,
                "edits[{index}].old_text 在文件中出现了 {count} 次，必须唯一。请包含更多上下文"
            ),
            Self::Overlap { first, second } => write!(
                f,
                "edits[{first}] 与 edits[{second}] 的范围重叠，请合并为一项或改为互不相交的区域"
            ),
            Self::Unchanged => f.write_str("替换后的内容与原文件相同，未做任何修改"),
        }
    }
}

/// 在原文上定位全部替换并应用
pub(crate) fn apply(
    content: &str,
    replacements: &[Replacement<'_>],
) -> Result<Applied, ApplyError> {
    let mut fuzzy_text: Option<FuzzyText> = None;
    let mut fuzzy = 0;
    let mut located: Vec<(Range<usize>, usize)> = Vec::with_capacity(replacements.len());
    for (index, replacement) in replacements.iter().enumerate() {
        let old = replacement.old_text;
        if old.is_empty() {
            return Err(ApplyError::EmptyOldText { index });
        }
        let range = match occurrences(content, old).as_slice() {
            [start] => *start..start + old.len(),
            [] => {
                let text = fuzzy_text.get_or_insert_with(|| FuzzyText::new(content));
                match text.find_unique(old) {
                    FuzzyMatch::Unique(range) => {
                        fuzzy += 1;
                        range
                    }
                    FuzzyMatch::Many(count) => return Err(ApplyError::Ambiguous { index, count }),
                    FuzzyMatch::None => return Err(ApplyError::NotFound { index }),
                }
            }
            many => {
                return Err(ApplyError::Ambiguous {
                    index,
                    count: many.len(),
                })
            }
        };
        located.push((range, index));
    }

    located.sort_by_key(|(range, _)| range.start);
    for pair in located.windows(2) {
        let ((before, a), (after, b)) = (&pair[0], &pair[1]);
        if before.end > after.start {
            return Err(ApplyError::Overlap {
                first: *a.min(b),
                second: *a.max(b),
            });
        }
    }

    let mut result = content.to_string();
    for (range, index) in located.iter().rev() {
        result.replace_range(range.clone(), replacements[*index].new_text);
    }
    if result == content {
        return Err(ApplyError::Unchanged);
    }
    Ok(Applied {
        content: result,
        fuzzy,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit<'a>(old_text: &'a str, new_text: &'a str) -> Replacement<'a> {
        Replacement { old_text, new_text }
    }

    #[test]
    fn edits_locate_against_the_original_and_apply_together() {
        let applied = apply("a1 b1 c1", &[edit("c1", "c2"), edit("a1", "a2 b1")]).unwrap();
        assert_eq!(applied.content, "a2 b1 b1 c2");
        assert_eq!(applied.fuzzy, 0);
    }

    #[test]
    fn exact_match_is_preferred_over_fuzzy() {
        let applied = apply("x = 'a';\n", &[edit("'a'", "'b'")]).unwrap();
        assert_eq!((applied.content.as_str(), applied.fuzzy), ("x = 'b';\n", 0));
    }

    #[test]
    fn fuzzy_fallback_replaces_only_the_matched_region() {
        let original = "keep  \nsay(“hi”);  \ntail\n";
        let applied = apply(original, &[edit("say(\"hi\");", "say(\"bye\");")]).unwrap();
        assert_eq!(applied.content, "keep  \nsay(\"bye\");  \ntail\n");
        assert_eq!(applied.fuzzy, 1);
    }

    #[test]
    fn failures_name_the_offending_edit() {
        assert_eq!(
            apply("abc", &[edit("a", "x"), edit("", "y")]),
            Err(ApplyError::EmptyOldText { index: 1 })
        );
        assert_eq!(
            apply("abc", &[edit("zz", "y")]),
            Err(ApplyError::NotFound { index: 0 })
        );
        assert_eq!(
            apply("ab ab", &[edit("ab", "x")]),
            Err(ApplyError::Ambiguous { index: 0, count: 2 })
        );
        assert_eq!(
            apply("abcdef", &[edit("cde", "1"), edit("abcd", "2")]),
            Err(ApplyError::Overlap {
                first: 0,
                second: 1
            })
        );
        assert_eq!(apply("abc", &[edit("b", "b")]), Err(ApplyError::Unchanged));
    }

    /// 相邻但不相交的区域不算重叠
    #[test]
    fn adjacent_regions_are_not_overlapping() {
        let applied = apply("abcd", &[edit("ab", "1"), edit("cd", "2")]).unwrap();
        assert_eq!(applied.content, "12");
    }
}
