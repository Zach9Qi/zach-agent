//! 工作区：工具可见的根目录、模型路径的解析与越界访问策略
//!
//! 路径一律做词法规范化，不访问文件系统、不跟随符号链接，因此可以搭配任意
//! [`crate::FileSystem`] 实现（本地、沙箱、远程）使用。

use std::path::{Component, Path, PathBuf};

/// 访问工作区之外路径时的策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutsideAccess {
    /// 直接放行
    Allow,
    /// 需要人工审批（经由 `zach-agent` 的审批流程）
    Ask,
    /// 拒绝，模型收到错误结果
    Deny,
}

/// 工具可见的工作区
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    read_outside: OutsideAccess,
    write_outside: OutsideAccess,
}

impl Workspace {
    /// 以根目录构造，根目录应为绝对路径
    ///
    /// 默认策略：读取工作区之外的路径直接放行，写入需要人工审批。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: normalize(&root.into()),
            read_outside: OutsideAccess::Allow,
            write_outside: OutsideAccess::Ask,
        }
    }

    /// 设置读取工作区之外路径的策略
    pub fn with_read_outside(mut self, access: OutsideAccess) -> Self {
        self.read_outside = access;
        self
    }

    /// 设置写入工作区之外路径的策略
    pub fn with_write_outside(mut self, access: OutsideAccess) -> Self {
        self.write_outside = access;
        self
    }

    /// 工作区根目录
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 把模型给出的路径解析为规范化的绝对路径
    ///
    /// 相对路径以根目录为基准；去掉模型常带的 `@` 前缀；Windows 上额外识别
    /// `/c/...`、`/mnt/c/...`、`/cygdrive/c/...` 这类 MSYS / WSL 风格的盘符路径。
    pub fn resolve(&self, path: &str) -> PathBuf {
        let path = path.trim();
        let path = path.strip_prefix('@').unwrap_or(path);
        let converted = if cfg!(windows) {
            msys_to_windows(path)
        } else {
            None
        };
        let path = Path::new(converted.as_deref().unwrap_or(path));
        if path.is_absolute() {
            normalize(path)
        } else {
            normalize(&self.root.join(path))
        }
    }

    /// 路径是否位于工作区内（词法判断，Windows 上不区分大小写）
    pub fn contains(&self, path: &Path) -> bool {
        let root = path_key(&self.root);
        let root = root.trim_end_matches('/');
        let path = path_key(path);
        path == root || path.starts_with(&format!("{root}/"))
    }

    /// 访问某路径适用的策略；工作区内恒为放行
    pub(crate) fn access(&self, path: &Path, write: bool) -> OutsideAccess {
        if self.contains(path) {
            OutsideAccess::Allow
        } else if write {
            self.write_outside
        } else {
            self.read_outside
        }
    }

    /// 给模型看的路径：工作区内为 `/` 分隔的相对路径（根目录为 `.`），之外为绝对路径
    pub(crate) fn display(&self, path: &Path) -> String {
        if !self.contains(path) {
            return path.display().to_string();
        }
        let relative: Vec<String> = path
            .components()
            .skip(self.root.components().count())
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        if relative.is_empty() {
            ".".to_string()
        } else {
            relative.join("/")
        }
    }
}

/// 路径比较用的键：统一 `/` 分隔，Windows 上转小写
pub(crate) fn path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// 词法规范化：消去 `.` 与 `..`，不越过根
fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match parts.last() {
                Some(Component::Normal(_)) => {
                    parts.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => parts.push(component),
            },
            _ => parts.push(component),
        }
    }
    parts.iter().collect()
}

/// 把 MSYS / WSL 风格的盘符路径转为 Windows 路径，不匹配时返回 `None`
fn msys_to_windows(path: &str) -> Option<String> {
    let rest = path
        .strip_prefix("/mnt/")
        .or_else(|| path.strip_prefix("/cygdrive/"))
        .or_else(|| path.strip_prefix('/'))?;
    let mut chars = rest.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let tail = chars.as_str();
    if !(tail.is_empty() || tail.starts_with('/')) {
        return None;
    }
    let tail = tail.trim_start_matches('/').replace('/', "\\");
    Some(format!("{}:\\{tail}", drive.to_ascii_uppercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msys_drive_paths_become_windows_paths() {
        assert_eq!(
            msys_to_windows("/c/Users/a").as_deref(),
            Some("C:\\Users\\a")
        );
        assert_eq!(msys_to_windows("/mnt/d/x/y").as_deref(), Some("D:\\x\\y"));
        assert_eq!(msys_to_windows("/cygdrive/e").as_deref(), Some("E:\\"));
        assert_eq!(msys_to_windows("/d/").as_deref(), Some("D:\\"));
    }

    #[test]
    fn ordinary_absolute_paths_are_not_mistaken_for_drives() {
        assert_eq!(msys_to_windows("/usr/bin"), None);
        assert_eq!(msys_to_windows("/c1/x"), None);
        assert_eq!(msys_to_windows("relative/c"), None);
        assert_eq!(msys_to_windows("/"), None);
    }

    #[test]
    fn normalize_collapses_dots_without_escaping_the_root() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };
        let path = Path::new(root).join("a/./b/../c/../../../d");
        assert_eq!(normalize(&path), Path::new(root).join("d"));
    }
}
