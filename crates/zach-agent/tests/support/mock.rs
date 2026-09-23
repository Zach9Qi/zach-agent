//! 测试支撑：脚本化模型与记录型宿主

#![allow(dead_code)]

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use zach_agent::{
    AgentEvent, AgentTool, ApprovalDecision, ApprovalRequest, LoopHost, SharedTool, ToolContext,
    ToolError, ToolExecutionMode, ToolOutcome,
};
use zach_ai_core::{
    CallOptions, FinishReason, FunctionTool, GenerateResult, LanguageModel, LanguageModelStream,
    Message, ModelError, StreamPart, UnifiedFinishReason, Usage,
};

/// 一次 `do_stream` 调用的脚本
pub enum Script {
    /// 依次产出分块后正常结束
    Parts(Vec<StreamPart>),
    /// 依次产出（可含传输错误）
    Items(Vec<Result<StreamPart, ModelError>>),
    /// 产出分块后挂起，直到被取消
    Hang(Vec<StreamPart>),
    /// 建立流之前失败
    Fail(ModelError),
}

/// 按脚本逐次响应的模型，并记录每次收到的调用参数
#[derive(Default)]
pub struct ScriptedModel {
    scripts: Mutex<VecDeque<Script>>,
    pub calls: Mutex<Vec<CallOptions>>,
}

impl ScriptedModel {
    pub fn new(scripts: Vec<Script>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn call(&self, index: usize) -> CallOptions {
        self.calls.lock().unwrap()[index].clone()
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
        self.calls.lock().unwrap().push(options);
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Script::Parts(text("（脚本已耗尽）")));
        match script {
            Script::Parts(parts) => Ok(Box::pin(futures::stream::iter(parts.into_iter().map(Ok)))),
            Script::Items(items) => Ok(Box::pin(futures::stream::iter(items))),
            Script::Hang(parts) => Ok(Box::pin(
                futures::stream::iter(parts.into_iter().map(Ok)).chain(futures::stream::pending()),
            )),
            Script::Fail(error) => Err(error),
        }
    }
}

pub fn finish(unified: UnifiedFinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::simple(10, 5),
        finish_reason: FinishReason { unified, raw: None },
        provider_metadata: None,
    }
}

/// 一段完整文本回复
pub fn text(content: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::TextStart {
            id: "t".into(),
            provider_metadata: None,
        },
        StreamPart::TextDelta {
            id: "t".into(),
            delta: content.into(),
            provider_metadata: None,
        },
        StreamPart::TextEnd {
            id: "t".into(),
            provider_metadata: None,
        },
        finish(UnifiedFinishReason::Stop),
    ]
}

pub fn tool_call(id: &str, name: &str, input: &str) -> StreamPart {
    StreamPart::ToolCall {
        tool_call_id: id.into(),
        tool_name: name.into(),
        input: input.into(),
        provider_executed: false,
        dynamic: false,
        provider_metadata: None,
    }
}

/// 若干工具调用后以 `reason` 结束
pub fn tool_calls(calls: &[(&str, &str, &str)], reason: UnifiedFinishReason) -> Vec<StreamPart> {
    let mut parts: Vec<StreamPart> = calls
        .iter()
        .map(|(id, name, input)| tool_call(id, name, input))
        .collect();
    parts.push(finish(reason));
    parts
}

type ToolFn =
    dyn Fn(Value, ToolContext) -> BoxFuture<'static, Result<ToolOutcome, ToolError>> + Send + Sync;

/// 由闭包实现的测试工具
pub struct FnTool {
    definition: FunctionTool,
    approval: bool,
    mode: Option<ToolExecutionMode>,
    run: Box<ToolFn>,
}

pub fn fn_tool<F, Fut>(name: &str, run: F) -> FnTool
where
    F: Fn(Value, ToolContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<ToolOutcome, ToolError>> + Send + 'static,
{
    FnTool {
        definition: FunctionTool::new(name, json!({ "type": "object" })),
        approval: false,
        mode: None,
        run: Box::new(move |input, ctx| Box::pin(run(input, ctx))),
    }
}

/// 原样返回入参的工具
pub fn echo_tool(name: &str) -> SharedTool {
    fn_tool(name, |input, _| async move { Ok(ToolOutcome::json(input)) }).shared()
}

impl FnTool {
    pub fn needs_approval(mut self) -> Self {
        self.approval = true;
        self
    }

    pub fn sequential(mut self) -> Self {
        self.mode = Some(ToolExecutionMode::Sequential);
        self
    }

    pub fn shared(self) -> SharedTool {
        Arc::new(self)
    }
}

#[async_trait]
impl AgentTool for FnTool {
    fn definition(&self) -> &FunctionTool {
        &self.definition
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.mode
    }

    fn needs_approval(&self, _input: &Value) -> bool {
        self.approval
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        (self.run)(input, ctx).await
    }
}

type ApprovalFn = Box<dyn Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync>;

/// 记录事件与写入消息的宿主；排队消息每次读取弹出一批
pub struct TestHost {
    pub events: Mutex<Vec<AgentEvent>>,
    pub messages: Mutex<Vec<Message>>,
    pub steering: Mutex<VecDeque<Vec<Message>>>,
    pub follow_up: Mutex<VecDeque<Vec<Message>>>,
    pub approvals: Mutex<Vec<ApprovalRequest>>,
    approve: ApprovalFn,
}

impl Default for TestHost {
    fn default() -> Self {
        Self::with_approval(|_| ApprovalDecision::approve())
    }
}

impl TestHost {
    pub fn with_approval(
        approve: impl Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync + 'static,
    ) -> Self {
        Self {
            events: Mutex::default(),
            messages: Mutex::default(),
            steering: Mutex::default(),
            follow_up: Mutex::default(),
            approvals: Mutex::default(),
            approve: Box::new(approve),
        }
    }

    pub fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }

    /// 事件判别值序列（如 `run-start`）
    pub fn kinds(&self) -> Vec<String> {
        kinds(&self.events())
    }
}

pub fn kinds(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .map(
            |event| match serde_json::to_value(event).unwrap()["type"].clone() {
                Value::String(kind) => kind,
                other => other.to_string(),
            },
        )
        .collect()
}

#[async_trait]
impl LoopHost for TestHost {
    async fn emit(&self, event: AgentEvent) {
        self.events.lock().unwrap().push(event);
    }

    async fn on_message(&self, message: &Message) {
        self.messages.lock().unwrap().push(message.clone());
    }

    async fn poll_steering(&self) -> Vec<Message> {
        self.steering
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default()
    }

    async fn poll_follow_up(&self) -> Vec<Message> {
        self.follow_up
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default()
    }

    async fn wait_approval(&self, request: ApprovalRequest) -> ApprovalDecision {
        let decision = (self.approve)(&request);
        self.approvals.lock().unwrap().push(request);
        decision
    }
}
