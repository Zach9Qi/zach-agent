//! 共享测试替身：纯内存文件系统、脚本化 shell、脚本化模型与工具调用辅助

use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use zach_agent::{SharedTool, ToolContext, ToolError, ToolOutcome};
use zach_ai_core::{
    CallOptions, FinishReason, GenerateResult, LanguageModel, LanguageModelStream, Message,
    ModelError, StreamPart, ToolResultOutput, Usage,
};
use zach_coding_agent::{
    CodingTools, FileInfo, FileSystem, FsEntry, OutputSink, Shell, ShellExit, ShellRequest,
    Workspace,
};

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

/// [`FakeShell`] 送完输出后的结局
#[derive(Debug, Clone, Copy)]
pub enum End {
    /// 以给定退出码结束
    Exit(Option<i32>),
    /// 永不结束（用于超时）
    Hang,
    /// 启动失败
    Fail,
}

/// 按脚本送出输出的 shell，记录收到的命令与工作目录
pub struct FakeShell {
    chunks: Vec<Vec<u8>>,
    end: End,
    requests: Mutex<Vec<(String, PathBuf)>>,
}

impl FakeShell {
    pub fn new(chunks: &[&str], end: End) -> Arc<Self> {
        Arc::new(Self {
            chunks: chunks
                .iter()
                .map(|chunk| chunk.as_bytes().to_vec())
                .collect(),
            end,
            requests: Mutex::default(),
        })
    }

    pub fn requests(&self) -> Vec<(String, PathBuf)> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl Shell for FakeShell {
    fn name(&self) -> &str {
        "bash"
    }

    async fn run(
        &self,
        request: ShellRequest<'_>,
        output: &mut OutputSink<'_>,
    ) -> io::Result<ShellExit> {
        let record = (request.command.to_string(), request.cwd.to_path_buf());
        self.requests.lock().unwrap().push(record);
        if let End::Fail = self.end {
            return Err(io::Error::other("找不到程序"));
        }
        for chunk in &self.chunks {
            output(chunk);
        }
        match self.end {
            End::Exit(code) => Ok(ShellExit { code }),
            End::Hang | End::Fail => std::future::pending().await,
        }
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

/// 执行时只记录"已执行"的工具
pub fn fn_tool_flag(name: &str) -> (Arc<std::sync::atomic::AtomicBool>, SharedTool) {
    let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool = Arc::new(FlagTool {
        definition: zach_ai_core::FunctionTool::new(name, serde_json::json!({ "type": "object" })),
        flag: flag.clone(),
    });
    (flag, tool)
}

struct FlagTool {
    definition: zach_ai_core::FunctionTool,
    flag: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl zach_agent::AgentTool for FlagTool {
    fn definition(&self) -> &zach_ai_core::FunctionTool {
        &self.definition
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolOutcome::text("已执行"))
    }
}

/// 按脚本逐次产出流式分块（或在建立流之前失败）的模型；脚本用完后挂起，记录每次请求的对话
#[derive(Default)]
pub struct ScriptedModel {
    scripts: Mutex<VecDeque<Result<Vec<StreamPart>, ModelError>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl ScriptedModel {
    pub fn new(scripts: Vec<Vec<StreamPart>>) -> Arc<Self> {
        Self::with_replies(scripts.into_iter().map(Ok).collect())
    }

    /// 每次请求依次取一个结果：成功时产出这些分块，失败时直接返回错误
    pub fn with_replies(replies: Vec<Result<Vec<StreamPart>, ModelError>>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(replies.into()),
            requests: Mutex::default(),
        })
    }

    /// 每次请求收到的对话（含系统提示词）
    pub fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LanguageModel for ScriptedModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "scripted"
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        Err(ModelError::unsupported("generate", None))
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        self.requests.lock().unwrap().push(options.prompt.messages);
        match self.scripts.lock().unwrap().pop_front() {
            Some(Ok(parts)) => Ok(Box::pin(futures::stream::iter(parts.into_iter().map(Ok)))),
            Some(Err(error)) => Err(error),
            None => Ok(Box::pin(futures::stream::pending())),
        }
    }
}

/// 一段完整的文本回复
pub fn reply_parts(text: &str) -> Vec<StreamPart> {
    let id = "t".to_string();
    vec![
        StreamPart::TextStart {
            id: id.clone(),
            provider_metadata: None,
        },
        StreamPart::TextDelta {
            id: id.clone(),
            delta: text.into(),
            provider_metadata: None,
        },
        StreamPart::TextEnd {
            id,
            provider_metadata: None,
        },
        finish(FinishReason::stop()),
    ]
}

/// 发起若干工具调用：`(tool_call_id, tool_name, 入参)`
pub fn call_parts(calls: &[(&str, &str, Value)]) -> Vec<StreamPart> {
    let mut parts: Vec<StreamPart> = calls
        .iter()
        .map(|(id, name, input)| StreamPart::ToolCall {
            tool_call_id: (*id).into(),
            tool_name: (*name).into(),
            input: input.to_string(),
            provider_executed: false,
            dynamic: false,
            provider_metadata: None,
        })
        .collect();
    parts.push(finish(FinishReason::tool_calls()));
    parts
}

fn finish(finish_reason: FinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::simple(100, 20),
        finish_reason,
        provider_metadata: None,
    }
}
