//! 对话记录提交通知：写入顺序、来源分类、回复的模型与用量、工具详情，以及中止与异常时不被跳过

use crate::support::{fn_tool, text, tool_calls, PanickingHooks, Script, ScriptedModel};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use zach_agent::{
    Agent, AgentBuilder, AgentEvent, Commit, CommitKind, RetryPolicy, ToolOutcome, TranscriptSink,
};
use zach_ai_core::{Message, UnifiedFinishReason};

/// 收到的一次提交
#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    kind: &'static str,
    message: Message,
    model: Option<(String, String)>,
    has_usage: bool,
    details: Vec<(String, Value)>,
}

#[derive(Default)]
struct Recorder {
    commits: Mutex<Vec<Recorded>>,
}

impl Recorder {
    fn commits(&self) -> Vec<Recorded> {
        self.commits.lock().unwrap().clone()
    }

    fn kinds(&self) -> Vec<&'static str> {
        self.commits().iter().map(|commit| commit.kind).collect()
    }
}

#[async_trait]
impl TranscriptSink for Recorder {
    async fn on_commit(&self, commit: &Commit<'_>) {
        let (kind, model, has_usage, details) = match commit.kind {
            CommitKind::Input => ("input", None, false, Vec::new()),
            CommitKind::Response(info) => {
                let model = (info.provider.to_string(), info.model_id.to_string());
                ("response", Some(model), info.usage.is_some(), Vec::new())
            }
            CommitKind::ToolResults { details } => ("tool_results", None, false, details.to_vec()),
            CommitKind::Recovery => ("recovery", None, false, Vec::new()),
        };
        self.commits.lock().unwrap().push(Recorded {
            kind,
            message: commit.message.clone(),
            model,
            has_usage,
            details,
        });
    }
}

fn builder(model: Arc<ScriptedModel>, recorder: &Arc<Recorder>) -> AgentBuilder {
    Agent::builder(model)
        .retry(RetryPolicy::none())
        .transcript(recorder.clone())
}

#[tokio::test]
async fn commits_follow_transcript_order_with_model_usage_and_tool_details() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "work", "{}"), ("c2", "plain", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("完成")),
    ]);
    let work = fn_tool("work", |_, _| async {
        Ok(ToolOutcome::text("ok").with_details(json!({ "exit_code": 0 })))
    });
    let plain = fn_tool("plain", |_, _| async { Ok(ToolOutcome::text("ok")) });
    let recorder = Arc::new(Recorder::default());
    let agent = builder(model, &recorder)
        .tools(vec![work.shared(), plain.shared()])
        .build();

    agent.prompt_text("开始").unwrap().outcome().await.unwrap();

    let commits = recorder.commits();
    assert_eq!(
        recorder.kinds(),
        ["input", "response", "tool_results", "response"]
    );
    let messages: Vec<Message> = commits.iter().map(|c| c.message.clone()).collect();
    assert_eq!(messages, agent.messages());
    assert_eq!(commits[1].model, Some(("mock".into(), "scripted".into())));
    assert!(commits[1].has_usage && commits[3].has_usage);
    assert_eq!(
        commits[2].details,
        [("c1".to_string(), json!({ "exit_code": 0 }))]
    );
}

/// 中止时已生成的部分回复照样通知，但没有用量
#[tokio::test]
async fn aborted_partial_responses_are_committed_without_usage() {
    let model = ScriptedModel::new(vec![Script::Hang(text("半")[..2].to_vec())]);
    let recorder = Arc::new(Recorder::default());
    let agent = builder(model, &recorder).build();

    let mut run = agent.prompt_text("你好").unwrap();
    while let Some(event) = run.next().await {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            agent.abort();
        }
    }

    let commits = recorder.commits();
    assert_eq!(recorder.kinds(), ["input", "response"]);
    assert_eq!(commits[1].message, Message::assistant("半"));
    assert!(!commits[1].has_usage);
}

#[tokio::test]
async fn panic_repairs_are_committed_as_recovery() {
    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &[("c1", "echo", "{}")],
        UnifiedFinishReason::ToolCalls,
    ))]);
    let recorder = Arc::new(Recorder::default());
    let agent = builder(model, &recorder)
        .hooks(PanickingHooks)
        .tool(crate::support::echo_tool("echo"))
        .build();

    let _ = agent.prompt_text("你好").unwrap().outcome().await;

    assert_eq!(recorder.kinds(), ["input", "response", "recovery"]);
    assert_eq!(
        recorder.commits()[2].message,
        *agent.messages().last().unwrap()
    );
}

#[tokio::test]
async fn transcript_sink_can_be_replaced_between_runs() {
    let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
    let first = Arc::new(Recorder::default());
    let agent = builder(model, &first).build();
    agent
        .prompt_text("第一问")
        .unwrap()
        .outcome()
        .await
        .unwrap();

    let second = Arc::new(Recorder::default());
    agent.set_transcript(Some(second.clone()));
    agent
        .prompt_text("第二问")
        .unwrap()
        .outcome()
        .await
        .unwrap();
    agent.set_transcript(None);

    assert_eq!(first.kinds(), ["input", "response"]);
    assert_eq!(second.commits()[0].message, Message::user("第二问"));
}
