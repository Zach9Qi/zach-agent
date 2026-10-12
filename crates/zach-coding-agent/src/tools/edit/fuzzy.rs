//! 宽松匹配：抹平模型常见的"看起来一样"的差异，并能把匹配位置映射回原文
//!
//! 规范化只做一对一或删除的字符替换，因此规范化文本的每个字节都能追溯到原文的某个字符：
//! - 弯引号、各类破折号与特殊空格换成 ASCII 对应字符；
//! - 删除行尾（换行或文本结尾之前）的空格与制表符。

use std::ops::Range;

/// 规范化后的文本，记录每个字节在原文中所属字符的范围
pub(crate) struct FuzzyText {
    text: String,
    starts: Vec<usize>,
    ends: Vec<usize>,
}

/// 在规范化空间中查找的结果
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FuzzyMatch {
    /// 唯一匹配，范围为原文中的字节区间
    Unique(Range<usize>),
    /// 匹配不止一处
    Many(usize),
    /// 没有匹配
    None,
}

impl FuzzyText {
    pub(crate) fn new(original: &str) -> Self {
        let mut normalized = Self {
            text: String::with_capacity(original.len()),
            starts: Vec::with_capacity(original.len()),
            ends: Vec::with_capacity(original.len()),
        };
        // 暂存的行内空白：遇到非空白字符才写出，遇到换行或结尾则丢弃
        let mut pending: Vec<(char, Range<usize>)> = Vec::new();
        for (offset, ch) in original.char_indices() {
            let span = offset..offset + ch.len_utf8();
            let ch = fold(ch);
            if ch == ' ' || ch == '\t' {
                pending.push((ch, span));
                continue;
            }
            if ch != '\n' {
                for (space, space_span) in pending.drain(..) {
                    normalized.push(space, space_span);
                }
            }
            pending.clear();
            normalized.push(ch, span);
        }
        normalized
    }

    fn push(&mut self, ch: char, span: Range<usize>) {
        for _ in 0..ch.len_utf8() {
            self.starts.push(span.start);
            self.ends.push(span.end);
        }
        self.text.push(ch);
    }

    /// 查找规范化后的 `needle`，唯一时返回其在原文中的范围
    pub(crate) fn find_unique(&self, needle: &str) -> FuzzyMatch {
        let needle = normalize(needle);
        if needle.is_empty() {
            return FuzzyMatch::None;
        }
        let found = occurrences(&self.text, &needle);
        match found.as_slice() {
            [] => FuzzyMatch::None,
            [start] => {
                let end = start + needle.len();
                FuzzyMatch::Unique(self.starts[*start]..self.ends[end - 1])
            }
            many => FuzzyMatch::Many(many.len()),
        }
    }
}

/// 按与 [`FuzzyText`] 相同的规则规范化
pub(crate) fn normalize(text: &str) -> String {
    FuzzyText::new(text).text
}

/// `needle` 在 `haystack` 中所有（可重叠）出现位置的起始字节
pub(crate) fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(index) = haystack[from..].find(needle) {
        let start = from + index;
        found.push(start);
        from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// 把"看起来一样"的 Unicode 变体折叠为 ASCII
fn fold(ch: char) -> char {
    match ch {
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' => ' ',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_lookalikes_fold_to_ascii() {
        assert_eq!(normalize("“a” ‘b’ x—y\u{a0}z"), "\"a\" 'b' x-y z");
    }

    #[test]
    fn trailing_whitespace_is_dropped_but_inner_whitespace_kept() {
        assert_eq!(normalize("a b  \n\tc\t\nd "), "a b\n\tc\nd");
    }

    /// 映射回原文的范围要覆盖被删掉的行尾空白与多字节字符
    #[test]
    fn matches_map_back_to_original_byte_ranges() {
        let original = "let s = “hi”;   \nnext";
        let fuzzy = FuzzyText::new(original);
        let FuzzyMatch::Unique(range) = fuzzy.find_unique("s = \"hi\";\nnext") else {
            panic!("应唯一匹配");
        };
        assert_eq!(&original[range], "s = “hi”;   \nnext");
    }

    #[test]
    fn ambiguity_counts_overlapping_occurrences() {
        assert_eq!(occurrences("aaa", "aa"), [0, 1]);
        assert_eq!(
            FuzzyText::new("x\u{a0}x x").find_unique("x x"),
            FuzzyMatch::Many(2)
        );
        assert_eq!(FuzzyText::new("abc").find_unique("  "), FuzzyMatch::None);
    }
}
