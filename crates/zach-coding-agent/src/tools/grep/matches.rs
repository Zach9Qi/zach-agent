//! 匹配结果的收集与排版：合并相邻的上下文区间，截断过长的行

use regex::Regex;

/// 单行最多展示的字符数
const MAX_LINE_CHARS: usize = 500;

/// 跨文件累积的匹配结果
pub(super) struct Matches {
    limit: usize,
    context: usize,
    count: usize,
    files: usize,
    lines: Vec<String>,
    clipped: bool,
}

impl Matches {
    pub(super) fn new(limit: usize, context: usize) -> Self {
        Self {
            limit,
            context,
            count: 0,
            files: 0,
            lines: Vec::new(),
            clipped: false,
        }
    }

    /// 已达到匹配数上限
    pub(super) fn is_full(&self) -> bool {
        self.count >= self.limit
    }

    pub(super) fn count(&self) -> usize {
        self.count
    }

    /// 有匹配的文件数
    pub(super) fn files(&self) -> usize {
        self.files
    }

    /// 是否有行因过长被截断
    pub(super) fn clipped(&self) -> bool {
        self.clipped
    }

    /// 排版好的输出行
    pub(super) fn lines(&self) -> &[String] {
        &self.lines
    }

    /// 搜索一个文件，匹配数不超过剩余额度
    pub(super) fn scan(&mut self, display: &str, text: &str, regex: &Regex) {
        let lines: Vec<&str> = text.lines().collect();
        let room = self.limit.saturating_sub(self.count);
        let hits: Vec<usize> = (0..lines.len())
            .filter(|&index| regex.is_match(lines[index]))
            .take(room)
            .collect();
        if hits.is_empty() {
            return;
        }
        self.count += hits.len();
        self.files += 1;

        for (start, end) in self.groups(&hits, lines.len()) {
            if self.context > 0 && !self.lines.is_empty() {
                self.lines.push("--".to_string());
            }
            for (index, line) in (start..=end).zip(&lines[start..=end]) {
                let mark = if hits.binary_search(&index).is_ok() {
                    ':'
                } else {
                    '-'
                };
                let content = self.clip(line);
                self.lines
                    .push(format!("{display}{mark}{}{mark} {content}", index + 1));
            }
        }
    }

    /// 匹配行连同上下文合并成互不相邻的闭区间
    fn groups(&self, hits: &[usize], total: usize) -> Vec<(usize, usize)> {
        let mut groups: Vec<(usize, usize)> = Vec::new();
        for &hit in hits {
            let start = hit.saturating_sub(self.context);
            let end = (hit + self.context).min(total - 1);
            match groups.last_mut() {
                Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
                _ => groups.push((start, end)),
            }
        }
        groups
    }

    fn clip(&mut self, line: &str) -> String {
        match line.char_indices().nth(MAX_LINE_CHARS) {
            Some((cut, _)) => {
                self.clipped = true;
                format!("{}…", &line[..cut])
            }
            None => line.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(context: usize, limit: usize, text: &str, pattern: &str) -> Matches {
        let mut matches = Matches::new(limit, context);
        matches.scan("f", text, &Regex::new(pattern).unwrap());
        matches
    }

    #[test]
    fn overlapping_context_windows_merge_into_one_group() {
        let matches = scan(1, 10, "a\nx\nb\nx\nc\nd\ne\nx", "x");
        assert_eq!(
            matches.lines(),
            ["f-1- a", "f:2: x", "f-3- b", "f:4: x", "f-5- c", "--", "f-7- e", "f:8: x"]
        );
    }

    #[test]
    fn matches_stop_at_the_remaining_quota() {
        let matches = scan(0, 2, "x\nx\nx", "x");
        assert_eq!(matches.lines(), ["f:1: x", "f:2: x"]);
        assert!(matches.is_full());
    }

    #[test]
    fn overlong_lines_are_clipped_on_char_boundaries() {
        let long = "中".repeat(MAX_LINE_CHARS + 5);
        let matches = scan(0, 10, &long, "中");
        assert!(matches.clipped());
        assert_eq!(
            matches.lines()[0].chars().count(),
            "f:1: ".len() + MAX_LINE_CHARS + 1
        );
    }
}
