//! 搜索类工具共用的 glob 匹配与路径整理

use globset::{GlobBuilder, GlobMatcher};
use std::path::Path;

/// 按 glob 匹配相对搜索目录的路径
///
/// - 不含 `/` 的模式只匹配文件名，如 `*.rs`；
/// - 含 `/` 的模式匹配相对路径，并允许出现在任意深度（自动补 `**/`），
///   以 `/` 开头则锚定在搜索目录下。
pub(crate) struct PathMatcher {
    name_only: bool,
    globs: Vec<GlobMatcher>,
}

impl PathMatcher {
    pub(crate) fn new(pattern: &str) -> Result<Self, globset::Error> {
        let pattern = pattern.trim().replace('\\', "/");
        let name_only = !pattern.contains('/');
        let mut sources = Vec::new();
        if let Some(anchored) = pattern.strip_prefix('/') {
            sources.push(anchored.to_string());
        } else {
            if !name_only && !pattern.starts_with("**/") {
                sources.push(format!("**/{pattern}"));
            }
            sources.push(pattern);
        }
        let globs = sources
            .iter()
            .map(|source| {
                GlobBuilder::new(source)
                    .literal_separator(true)
                    .build()
                    .map(|glob| glob.compile_matcher())
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { name_only, globs })
    }

    /// `relative` 为以 `/` 分隔、相对搜索目录的路径
    pub(crate) fn matches(&self, relative: &str) -> bool {
        let subject = if self.name_only {
            relative.rsplit('/').next().unwrap_or(relative)
        } else {
            relative
        };
        self.globs.iter().any(|glob| glob.is_match(subject))
    }
}

/// `path` 相对 `base` 的路径，以 `/` 分隔
pub(crate) fn relative_to(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matcher(pattern: &str) -> PathMatcher {
        PathMatcher::new(pattern).unwrap()
    }

    #[test]
    fn patterns_without_slashes_match_file_names_at_any_depth() {
        let rs = matcher("*.rs");
        assert!(rs.matches("main.rs"));
        assert!(rs.matches("src/deep/lib.rs"));
        assert!(!rs.matches("src/lib.rs.bak"));
    }

    #[test]
    fn patterns_with_slashes_match_paths_at_any_depth() {
        let nested = matcher("src/*.rs");
        assert!(nested.matches("src/lib.rs"));
        assert!(nested.matches("crates/a/src/lib.rs"));
        assert!(!nested.matches("src/deep/lib.rs"));
        assert!(matcher("src/**/*.rs").matches("src/deep/lib.rs"));
    }

    #[test]
    fn leading_slash_anchors_to_the_search_directory() {
        let anchored = matcher("/src/*.rs");
        assert!(anchored.matches("src/lib.rs"));
        assert!(!anchored.matches("crates/a/src/lib.rs"));
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        assert!(PathMatcher::new("src/[a").is_err());
    }
}
