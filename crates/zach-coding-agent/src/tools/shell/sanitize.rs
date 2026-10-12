//! 终端输出清洗：去掉控制序列，按终端语义处理回车，只留下模型需要的纯文本

use std::iter::Peekable;
use std::str::Chars;

/// 去掉 ANSI 转义序列与控制字符；行内的回车视为"回到行首覆盖"，只保留最后一段
pub(super) fn sanitize(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => skip_escape(&mut chars),
            '\n' | '\t' | '\r' => plain.push(ch),
            ch if ch.is_control() => {}
            ch => plain.push(ch),
        }
    }
    if !plain.contains('\r') {
        return plain;
    }
    plain
        .split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 跳过 ESC 之后的序列：CSI 到终止字节为止，OSC 等字符串序列到 BEL 或 ST 为止
fn skip_escape(chars: &mut Peekable<Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for ch in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&ch) {
                    break;
                }
            }
        }
        Some(']' | 'P' | 'X' | '^' | '_') => {
            while let Some(ch) = chars.next() {
                if ch == '\u{7}' {
                    break;
                }
                if ch == '\u{1b}' {
                    chars.next_if_eq(&'\\');
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_codes_and_hyperlinks_are_removed() {
        let colored =
            "\u{1b}[1;31merror\u{1b}[0m: \u{1b}]8;;https://x\u{7}link\u{1b}]8;;\u{1b}\\ done";
        assert_eq!(sanitize(colored), "error: link done");
    }

    #[test]
    fn carriage_returns_keep_the_final_overwrite() {
        assert_eq!(sanitize("10%\r50%\r100%\nnext\r\n"), "100%\nnext\n");
    }

    #[test]
    fn other_control_characters_are_dropped_but_tabs_kept() {
        assert_eq!(sanitize("a\u{8}\u{7}\tb\u{0}"), "a\tb");
    }

    /// 输出末尾未终止的转义序列只丢弃其自身
    #[test]
    fn unterminated_sequences_only_swallow_their_own_tail() {
        assert_eq!(sanitize("ok\u{1b}[31"), "ok");
        assert_eq!(sanitize("ok\u{1b}"), "ok");
    }
}
