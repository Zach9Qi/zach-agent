//! 共享测试替身：纯内存文件系统与工具调用辅助

use async_trait::async_trait;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use zach_agent::{SharedTool, ToolContext, ToolError, ToolOutcome};
use zach_ai_core::ToolResultOutput;
use zach_coding_agent::{CodingTools, FileInfo, FileSystem, FsEntry, Workspace};

/// 当前平台下的绝对根目录
pub fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\ws")
    } else {
        PathBuf::from("/ws")
    }
}

/// 工作区内的绝对路径，`relative` 以 `/` 分隔
pub fn at(relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root(), |path, part| path.join(part))
}

#[derive(Debug, Clone)]
enum Node {
    File(Vec<u8>),
    Dir,
}

/// 纯内存文件系统；遍历不实现忽略规则
#[derive(Debug, Default)]
pub struct MemoryFs {
    nodes: Mutex<BTreeMap<PathBuf, Node>>,
}

impl MemoryFs {
    /// 以工作区根目录为唯一目录构造
    pub fn new() -> Arc<Self> {
        let fs = Self::default();
        fs.insert_dirs(&root());
        Arc::new(fs)
    }

    /// 直接放入文件（自动补齐父目录）
    pub fn put(&self, path: &Path, data: impl AsRef<[u8]>) {
        if let Some(parent) = path.parent() {
            self.insert_dirs(parent);
        }
        let mut nodes = self.nodes.lock().unwrap();
        nodes.insert(path.to_path_buf(), Node::File(data.as_ref().to_vec()));
    }

    /// 文件内容（UTF-8）；不存在时为 `None`
    pub fn text(&self, path: &Path) -> Option<String> {
        match self.nodes.lock().unwrap().get(path) {
            Some(Node::File(data)) => Some(String::from_utf8(data.clone()).unwrap()),
            _ => None,
        }
    }

    fn insert_dirs(&self, path: &Path) {
        let mut nodes = self.nodes.lock().unwrap();
        for ancestor in path.ancestors() {
            nodes.entry(ancestor.to_path_buf()).or_insert(Node::Dir);
        }
    }

    fn entries(&self, dir: &Path, recursive: bool) -> io::Result<Vec<FsEntry>> {
        let nodes = self.nodes.lock().unwrap();
        match nodes.get(dir) {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(io::Error::other("不是目录")),
            None => return Err(io::ErrorKind::NotFound.into()),
        }
        Ok(nodes
            .iter()
            .filter(|(path, _)| path.as_path() != dir && path.starts_with(dir))
            .filter(|(path, _)| recursive || path.parent() == Some(dir))
            .map(|(path, node)| FsEntry {
                path: path.clone(),
                is_dir: matches!(node, Node::Dir),
            })
            .collect())
    }
}

#[async_trait]
impl FileSystem for MemoryFs {
    async fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.nodes.lock().unwrap().get(path) {
            Some(Node::File(data)) => Ok(data.clone()),
            Some(Node::Dir) => Err(io::Error::other("是目录")),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    async fn write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let mut nodes = self.nodes.lock().unwrap();
        let parent = path.parent().and_then(|parent| nodes.get(parent));
        if !matches!(parent, Some(Node::Dir)) {
            return Err(io::ErrorKind::NotFound.into());
        }
        nodes.insert(path.to_path_buf(), Node::File(data.to_vec()));
        Ok(())
    }

    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        self.insert_dirs(path);
        Ok(())
    }

    async fn metadata(&self, path: &Path) -> io::Result<FileInfo> {
        match self.nodes.lock().unwrap().get(path) {
            Some(Node::File(data)) => Ok(FileInfo {
                is_dir: false,
                len: data.len() as u64,
            }),
            Some(Node::Dir) => Ok(FileInfo {
                is_dir: true,
                len: 0,
            }),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    async fn read_dir(&self, path: &Path) -> io::Result<Vec<FsEntry>> {
        self.entries(path, false)
    }

    async fn walk(&self, path: &Path) -> io::Result<Vec<FsEntry>> {
        self.entries(path, true)
    }
}

/// 以内存文件系统构造的工具集
pub fn tools(fs: &Arc<MemoryFs>) -> CodingTools {
    CodingTools::new(Workspace::new(root()), fs.clone())
}

/// 按名称取出工具
pub fn tool(tools: &[SharedTool], name: &str) -> SharedTool {
    tools
        .iter()
        .find(|tool| tool.name() == name)
        .unwrap_or_else(|| panic!("缺少工具 {name}"))
        .clone()
}

/// 走完整的入参校验后执行一次工具调用
pub async fn call(tool: &SharedTool, input: Value) -> Result<ToolOutcome, ToolError> {
    let input = tool.prepare_input(input)?;
    tool.execute(input, ToolContext::new("c1", CancellationToken::new()))
        .await
}

/// 结果中的文本；错误结果也返回其文本
pub fn text_of(outcome: &ToolOutcome) -> &str {
    match &outcome.output {
        ToolResultOutput::Text { value, .. } | ToolResultOutput::ErrorText { value, .. } => value,
        other => panic!("不是文本结果: {other:?}"),
    }
}

/// 详情中的某个字段
pub fn detail<'a>(outcome: &'a ToolOutcome, key: &str) -> &'a Value {
    &outcome.details.as_ref().expect("缺少详情")[key]
}
