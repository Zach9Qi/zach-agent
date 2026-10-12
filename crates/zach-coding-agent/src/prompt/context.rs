//! 项目指令文件的发现：从工作区根目录逐级向上查找 AGENTS.md 等文件
//!
//! 经由 [`FileSystem`] 读取，可以搭配沙箱或远程文件系统；运行时不假设任何全局目录，
//! 用户级的指令目录由宿主经 [`ContextFiles::with_global_dir`] 提供。

use crate::fs::FileSystem;
use crate::workspace::path_key;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

/// 默认的指令文件名，同一目录内按顺序取第一个存在且非空的
const DEFAULT_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

/// 一个项目指令文件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    /// 去掉 BOM 与末尾空白后的内容
    pub content: String,
}

/// 项目指令文件的查找规则
#[derive(Debug, Clone)]
pub struct ContextFiles {
    names: Vec<String>,
    global_dir: Option<PathBuf>,
}

impl Default for ContextFiles {
    fn default() -> Self {
        Self {
            names: DEFAULT_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            global_dir: None,
        }
    }
}

impl ContextFiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// 同一目录内依次尝试的文件名，默认 `AGENTS.md`、`CLAUDE.md`
    pub fn with_names(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.names = names.into_iter().map(Into::into).collect();
        self
    }

    /// 宿主的全局指令目录（如用户配置目录），其中的文件排在最前
    pub fn with_global_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.global_dir = Some(dir.into());
        self
    }

    /// 查找指令文件：全局目录的在前，其后从最外层的祖先目录到 `root` 本身，越靠后越具体
    ///
    /// 每个目录至多取一个文件；读取失败的文件被跳过并记录警告，同一文件只出现一次。
    pub async fn discover(&self, fs: &dyn FileSystem, root: &Path) -> Vec<ContextFile> {
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        if let Some(dir) = &self.global_dir {
            if let Some(file) = self.load(fs, dir).await {
                seen.insert(path_key(&file.path));
                files.push(file);
            }
        }
        let mut ancestors = Vec::new();
        for dir in root.ancestors().filter(|dir| !dir.as_os_str().is_empty()) {
            if let Some(file) = self.load(fs, dir).await {
                if seen.insert(path_key(&file.path)) {
                    ancestors.push(file);
                }
            }
        }
        files.extend(ancestors.into_iter().rev());
        files
    }

    /// 目录中第一个存在且非空的指令文件
    async fn load(&self, fs: &dyn FileSystem, dir: &Path) -> Option<ContextFile> {
        for name in &self.names {
            let path = dir.join(name);
            match fs.metadata(&path).await {
                Ok(info) if !info.is_dir => {}
                Ok(_) => continue,
                Err(error) => {
                    warn_unreadable(&path, &error);
                    continue;
                }
            }
            let bytes = match fs.read(&path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    warn_unreadable(&path, &error);
                    continue;
                }
            };
            let text = String::from_utf8_lossy(&bytes);
            let content = text.strip_prefix('\u{feff}').unwrap_or(&text).trim_end();
            if !content.trim_start().is_empty() {
                let content = content.to_string();
                return Some(ContextFile { path, content });
            }
        }
        None
    }
}

fn warn_unreadable(path: &Path, error: &io::Error) {
    if error.kind() != io::ErrorKind::NotFound {
        tracing::warn!(path = %path.display(), %error, "无法读取项目指令文件，已跳过");
    }
}
