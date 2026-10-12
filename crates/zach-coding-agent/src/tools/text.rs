//! 文本处理的共用小工具

/// 判断二进制内容时检查的前缀长度
const BINARY_SNIFF_BYTES: usize = 8000;

/// 前缀中出现 NUL 字节即视为二进制（与 git 的判定方式一致）
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|&byte| byte == 0)
}

/// 去掉开头的 UTF-8 BOM，返回剩余文本与是否带 BOM
pub(crate) fn strip_bom(text: &str) -> (&str, bool) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => (rest, true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nul_bytes_mark_binary_content() {
        assert!(is_binary(b"\x89PNG\x00\x01"));
        assert!(!is_binary("中文 text\r\n".as_bytes()));
        let mut late = vec![b'a'; BINARY_SNIFF_BYTES];
        late.push(0);
        assert!(!is_binary(&late));
    }

    #[test]
    fn bom_is_stripped_and_reported() {
        assert_eq!(strip_bom("\u{feff}abc"), ("abc", true));
        assert_eq!(strip_bom("abc"), ("abc", false));
    }
}
