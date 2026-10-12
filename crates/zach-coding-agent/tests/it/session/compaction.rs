//! 上下文压缩：手动压缩与文件追踪、在上一次摘要基础上更新、运行前与轮次之间的阈值压缩、
//! 上下文超长后压缩并只重试一次、摘要失败

use crate::support::{at, call_parts, reply_parts, tools, MemoryFs, ScriptedModel};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::sync::{Arc, Mutex};
use zach_agent::{Agent, AgentEvent, RetryPolicy, SharedTool};
use zach_ai_core::{ErrorKind, Message, ModelError, UserPart};
use zach_coding_agent::session::{
    CompactionEvent, CompactionOptions, CompactionReason, EntryBody, MemoryStore, Session,
    SessionError, SessionEvent, SessionOptions, Summarizer, SummaryRequest,
};

/// 收到的摘要请求：被压缩的消息与上一次摘要
type Calls = Arc<Mutex<Vec<(Vec<Message>, Option<String>)>>>;

/// 依次返回"摘要1""摘要2"……的摘要器；`fail` 时一律失败
#[derive(Clone, Default)]
struct FakeSummarizer {
    calls: Calls,
    fail: bool,
}

#[async_trait]
impl Summarizer for FakeSummarizer {
    async fn summarize(&self, request: SummaryRequest<'_>) -> Result<String, ModelError> {
        if self.fail {
            return Err(ModelError::new(ErrorKind::RateLimited, "摘要服务繁忙"));
        }
        let mut calls = self.calls.lock().unwrap();
        calls.push((
            request.messages.to_vec(),
            request.previous.map(str::to_string),
        ));
        Ok(format!("摘要{}", calls.len()))
    }
}

fn agent(model: &Arc<ScriptedModel>, tools: Vec<SharedTool>) -> Agent {
    Agent::builder(model.clone())
        .retry(RetryPolicy::none())
        .tools(tools)
        .build()
}

fn session(
    model: &Arc<ScriptedModel>,
    tools: Vec<SharedTool>,
    compaction: CompactionOptions,
) -> Session {
    let options = SessionOptions::new().with_compaction(compaction);
    Session::create(Arc::new(MemoryStore::new()), agent(model, tools), options)
}

/// 用户消息的文本
fn user_text(message: &Message) -> String {
    let Message::User { content, .. } = message else {
        panic!("不是用户消息: {message:?}");
    };
    content
        .iter()
        .filter_map(|part| match part {
            UserPart::Text { text, .. } => Some(text.as_str()),
            UserPart::File { .. } => None,
        })
        .collect()
}

fn compaction_events(events: &[SessionEvent]) -> Vec<CompactionEvent> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Compaction(event) => Some(event.clone()),
            _ => None,
        })
        .collect()
}

async fn run_text(session: &Session, text: &str) -> Vec<SessionEvent> {
    let mut run = session.prompt_text(text).unwrap();
    let events: Vec<SessionEvent> = run.by_ref().collect().await;
    run.outcome().await.unwrap();
    events
}

/// 读过一个文件、又问了一个问题的会话；保留量 3 token 恰好保留最后一问一答
async fn read_then_ask(summarizer: &FakeSummarizer) -> Session {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "1\n");
    let model = ScriptedModel::new(vec![
        call_parts(&[("c1", "read", json!({ "path": "a.txt" }))]),
        reply_parts("是 1"),
        reply_parts("二答"),
        reply_parts("三答"),
    ]);
    let compaction = CompactionOptions::new()
        .with_keep_recent_tokens(3)
        .with_summarizer(summarizer.clone());
    let session = session(&model, tools(&fs).coding(), compaction);
    run_text(&session, "读 a.txt").await;
    run_text(&session, "二").await;
    session
}

#[tokio::test]
async fn manual_compaction_summarizes_older_turns_and_tracks_files() {
    let summarizer = FakeSummarizer::default();
    let session = read_then_ask(&summarizer).await;

    let id = session.compact().await.unwrap().expect("应有可压缩的内容");
    let entries = session.entries();
    let EntryBody::Compaction(entry) = &entries.last().unwrap().body else {
        panic!("最后一条应为压缩条目");
    };
    assert_eq!(entries.last().unwrap().id, id);
    assert_eq!(entry.reason, CompactionReason::Manual);
    assert_eq!(entry.read_files, ["a.txt"]);
    assert!(entry.summary.starts_with("摘要1") && entry.summary.contains("<read-files>\na.txt"));

    let (summarized, previous) = summarizer.calls.lock().unwrap()[0].clone();
    assert_eq!(summarized.len(), 4);
    assert_eq!(previous, None);

    let context = session.context().await.unwrap();
    assert!(user_text(&context[0]).contains("<summary>\n摘要1"));
    assert_eq!(
        &context[1..],
        [Message::user("二"), Message::assistant("二答")]
    );
}

#[tokio::test]
async fn later_compactions_update_the_previous_summary_and_keep_file_lists() {
    let summarizer = FakeSummarizer::default();
    let session = read_then_ask(&summarizer).await;
    session.compact().await.unwrap();
    run_text(&session, "三").await;

    session.compact().await.unwrap();
    let (summarized, previous) = summarizer.calls.lock().unwrap()[1].clone();
    assert!(previous.unwrap().starts_with("摘要1"));
    assert_eq!(
        summarized,
        [Message::user("二"), Message::assistant("二答")]
    );
    let entries = session.entries();
    let EntryBody::Compaction(entry) = &entries.last().unwrap().body else {
        panic!("最后一条应为压缩条目");
    };
    assert_eq!(entry.read_files, ["a.txt"]);
}

#[tokio::test]
async fn compaction_needs_more_history_than_the_kept_region() {
    let model = ScriptedModel::new(vec![reply_parts("答")]);
    let session = session(&model, Vec::new(), CompactionOptions::new());
    assert_eq!(session.compact().await.unwrap(), None);
    run_text(&session, "问").await;
    assert_eq!(session.compact().await.unwrap(), None);
}

/// 上一轮的用量（120 token）已超过"窗口 40 − 预留 10"，下一次提问前先压缩
#[tokio::test]
async fn threshold_compaction_runs_before_the_next_prompt() {
    let model = ScriptedModel::new(vec![reply_parts("一答"), reply_parts("二答")]);
    let compaction = CompactionOptions::new()
        .with_context_window(40)
        .with_reserve_tokens(10)
        .with_keep_recent_tokens(3)
        .with_summarizer(FakeSummarizer::default());
    let session = session(&model, Vec::new(), compaction);
    assert!(compaction_events(&run_text(&session, "一").await).is_empty());

    let events = run_text(&session, "二").await;
    assert!(matches!(
        compaction_events(&events).as_slice(),
        [
            CompactionEvent::Started {
                reason: CompactionReason::Threshold,
                tokens_before: 120
            },
            CompactionEvent::Finished {
                reason: CompactionReason::Threshold,
                ..
            },
        ]
    ));
    let request = model.requests().pop().unwrap();
    assert!(user_text(&request[0]).contains("摘要1"));
    assert_eq!(request.last(), Some(&Message::user("二")));
}

/// 工具调用那一轮的用量已超阈值：同一次运行的下一个请求换成压缩后的上下文
#[tokio::test]
async fn threshold_compaction_between_turns_replaces_the_request_context() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "1\n");
    let model = ScriptedModel::new(vec![
        call_parts(&[("c1", "read", json!({ "path": "a.txt" }))]),
        reply_parts("是 1"),
    ]);
    let compaction = CompactionOptions::new()
        .with_context_window(40)
        .with_reserve_tokens(10)
        .with_keep_recent_tokens(3)
        .with_summarizer(FakeSummarizer::default());
    let session = session(&model, tools(&fs).coding(), compaction);

    let events = run_text(&session, "读 a.txt").await;
    assert_eq!(compaction_events(&events).len(), 2);
    let second = &model.requests()[1];
    assert!(user_text(&second[0]).contains("摘要1"));
    assert!(matches!(second[1], Message::Assistant { .. }));
    assert!(matches!(second[2], Message::Tool { .. }));
}

fn overflow() -> ModelError {
    ModelError::new(ErrorKind::ContextLengthExceeded, "上下文超长")
}

/// 超长失败被压缩后的继续运行接住，使用方看不到这次 RunError
#[tokio::test]
async fn context_overflow_compacts_and_continues_once() {
    let model = ScriptedModel::with_replies(vec![
        Ok(reply_parts("一答")),
        Err(overflow()),
        Ok(reply_parts("二答")),
    ]);
    let compaction = CompactionOptions::new()
        .with_keep_recent_tokens(3)
        .with_summarizer(FakeSummarizer::default());
    let session = session(&model, Vec::new(), compaction);
    run_text(&session, "一").await;

    let events = run_text(&session, "二").await;
    assert!(!events
        .iter()
        .any(|event| matches!(event, SessionEvent::Agent(AgentEvent::RunError { .. }))));
    assert!(matches!(
        compaction_events(&events).as_slice(),
        [
            CompactionEvent::Started {
                reason: CompactionReason::Overflow,
                ..
            },
            CompactionEvent::Finished {
                reason: CompactionReason::Overflow,
                ..
            },
        ]
    ));
    assert_eq!(
        session.context().await.unwrap().last(),
        Some(&Message::assistant("二答"))
    );
}

#[tokio::test]
async fn a_second_overflow_is_reported() {
    let model = ScriptedModel::with_replies(vec![
        Ok(reply_parts("一答")),
        Err(overflow()),
        Err(overflow()),
    ]);
    let compaction = CompactionOptions::new()
        .with_keep_recent_tokens(3)
        .with_summarizer(FakeSummarizer::default());
    let session = session(&model, Vec::new(), compaction);
    run_text(&session, "一").await;

    let mut run = session.prompt_text("二").unwrap();
    let events: Vec<SessionEvent> = run.by_ref().collect().await;
    let result = run.outcome().await;
    assert!(
        matches!(result, Err(SessionError::Run(error)) if error.kind() == ErrorKind::ContextLengthExceeded)
    );
    let errors = events
        .iter()
        .filter(|event| matches!(event, SessionEvent::Agent(AgentEvent::RunError { .. })))
        .count();
    assert_eq!(errors, 1);
}

#[tokio::test]
async fn summarizer_failures_leave_the_log_unchanged() {
    let summarizer = FakeSummarizer {
        fail: true,
        ..FakeSummarizer::default()
    };
    let session = read_then_ask(&summarizer).await;
    let before = session.entries().len();
    assert!(matches!(
        session.compact().await,
        Err(SessionError::Compaction(reason)) if reason.contains("摘要服务繁忙")
    ));
    assert_eq!(session.entries().len(), before);
    assert!(session.failure().is_none());
}
