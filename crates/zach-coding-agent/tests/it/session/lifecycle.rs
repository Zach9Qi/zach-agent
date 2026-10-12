//! 会话运行生命周期：逐条写入与恢复、懒创建、配置变化、回退与分叉、大对象外置、写入失败与并发保护

use super::FailingStore;
use crate::support::{at, call_parts, fn_tool_flag, reply_parts, tools, MemoryFs, ScriptedModel};
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use zach_agent::{Agent, RetryPolicy, SharedTool};
use zach_ai_core::{FileData, Message, ReasoningEffort, UserPart};
use zach_coding_agent::session::{
    EntryBody, ForkOrigin, LogError, MemoryStore, MessageOrigin, Session, SessionError,
    SessionEvent, SessionOptions, SessionStore,
};

fn agent(model: &Arc<ScriptedModel>, tools: Vec<SharedTool>) -> Agent {
    Agent::builder(model.clone())
        .system_prompt("你是编码助手")
        .retry(RetryPolicy::none())
        .tools(tools)
        .build()
}

/// 已落盘条目的类别：消息条目取其来源，其他取条目类型
async fn kinds(store: &MemoryStore, session: &Session) -> Vec<String> {
    let stored = store.load(session.id()).await.unwrap();
    stored
        .entries
        .iter()
        .map(|entry| match &entry.body {
            EntryBody::Message(message) => format!("{:?}", message.origin).to_lowercase(),
            EntryBody::Compaction(_) => "compaction".into(),
            EntryBody::Config(_) => "config".into(),
            EntryBody::Custom(_) => "custom".into(),
            EntryBody::Checkout { .. } => "checkout".into(),
        })
        .collect()
}

#[tokio::test]
async fn every_commit_is_stored_and_a_reopened_session_resumes() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "1\n");
    let model = ScriptedModel::new(vec![
        call_parts(&[("c1", "read", json!({ "path": "a.txt" }))]),
        reply_parts("是 1"),
        reply_parts("记得"),
    ]);
    let store = Arc::new(MemoryStore::new());
    let options = SessionOptions::new().with_workspace_root("/repo");
    let session = Session::create(
        store.clone(),
        agent(&model, tools(&fs).coding()),
        options.clone(),
    );
    session
        .prompt_text("a.txt 里是什么")
        .unwrap()
        .outcome()
        .await
        .unwrap();

    assert_eq!(
        kinds(&store, &session).await,
        ["config", "input", "response", "toolresults", "response"]
    );
    let stored = store.load(session.id()).await.unwrap();
    assert_eq!(stored.header.workspace_root.as_deref(), Some("/repo"));
    let EntryBody::Message(response) = &stored.entries[2].body else {
        panic!("第 3 条应为回复");
    };
    assert_eq!(response.model.as_ref().unwrap().id, "scripted");
    assert!(response.usage.is_some());
    let EntryBody::Message(results) = &stored.entries[3].body else {
        panic!("第 4 条应为工具结果");
    };
    assert_eq!(results.tool_details["c1"]["path"], "a.txt");

    let reopened = Session::open(
        store.clone(),
        session.id(),
        agent(&model, tools(&fs).coding()),
        options,
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.context().await.unwrap(),
        session.context().await.unwrap()
    );
    reopened
        .prompt_text("还记得吗")
        .unwrap()
        .outcome()
        .await
        .unwrap();

    let last = model.requests().pop().unwrap();
    assert_eq!(last[last.len() - 2], Message::assistant("是 1"));
    assert_eq!(last[last.len() - 1], Message::user("还记得吗"));
    let configs = kinds(&store, &reopened)
        .await
        .iter()
        .filter(|kind| *kind == "config")
        .count();
    assert_eq!(configs, 1, "配置没有变化时不应重复记录");
}

#[tokio::test]
async fn nothing_is_stored_before_the_first_message() {
    let model = ScriptedModel::new(vec![reply_parts("好")]);
    let store = Arc::new(MemoryStore::new());
    let session = Session::create(
        store.clone(),
        agent(&model, Vec::new()),
        SessionOptions::new(),
    );
    session
        .append_custom("host.ui", json!({ "pinned": true }))
        .await
        .unwrap();
    assert!(store.list().await.unwrap().is_empty());

    session
        .prompt_text("你好")
        .unwrap()
        .outcome()
        .await
        .unwrap();
    assert_eq!(store.list().await.unwrap().len(), 1);
    assert_eq!(
        kinds(&store, &session).await,
        ["custom", "config", "input", "response"]
    );
}

#[tokio::test]
async fn config_changes_record_only_the_changed_fields() {
    let model = ScriptedModel::new(vec![reply_parts("一"), reply_parts("二")]);
    let store = Arc::new(MemoryStore::new());
    let agent = agent(&model, Vec::new());
    let session = Session::create(store.clone(), agent.clone(), SessionOptions::new());
    session.prompt_text("一").unwrap().outcome().await.unwrap();
    agent.set_system_prompt(Some("新提示词".into()));
    agent.set_reasoning(Some(ReasoningEffort::High));
    session.prompt_text("二").unwrap().outcome().await.unwrap();

    let stored = store.load(session.id()).await.unwrap();
    let EntryBody::Config(first) = &stored.entries[0].body else {
        panic!("第 1 条应为配置: {:?}", stored.entries[0]);
    };
    assert!(first.reasoning.is_none(), "未设置推理档位时不记录");
    let EntryBody::Config(change) = &stored.entries[3].body else {
        panic!("第 4 条应为配置变化: {:?}", stored.entries[3]);
    };
    assert_eq!(change.system_prompt.as_deref(), Some("新提示词"));
    assert_eq!(change.reasoning, Some(ReasoningEffort::High));
    assert!(change.model.is_none() && change.tools.is_none());
}

#[tokio::test]
async fn rewind_starts_a_new_branch_that_survives_reopening() {
    let model = ScriptedModel::new(vec![
        reply_parts("一答"),
        reply_parts("二答"),
        reply_parts("三答"),
    ]);
    let store = Arc::new(MemoryStore::new());
    let session = Session::create(
        store.clone(),
        agent(&model, Vec::new()),
        SessionOptions::new(),
    );
    session.prompt_text("一").unwrap().outcome().await.unwrap();
    session.prompt_text("二").unwrap().outcome().await.unwrap();

    session.rewind(3).await.unwrap();
    session.prompt_text("三").unwrap().outcome().await.unwrap();
    let expected = [
        Message::user("一"),
        Message::assistant("一答"),
        Message::user("三"),
        Message::assistant("三答"),
    ];
    assert_eq!(session.context().await.unwrap(), expected);
    assert!(!model.requests()[2].contains(&Message::user("二")));

    let reopened = Session::open(
        store,
        session.id(),
        agent(&model, Vec::new()),
        SessionOptions::new(),
    )
    .await
    .unwrap();
    assert_eq!(reopened.leaf(), session.leaf());
    assert_eq!(reopened.context().await.unwrap(), expected);
    assert!(matches!(
        session.rewind(99).await,
        Err(SessionError::Log(LogError::InvalidCheckout { target: 99 }))
    ));
}

#[tokio::test]
async fn fork_copies_the_branch_into_a_new_session() {
    let model = ScriptedModel::new(vec![reply_parts("一答"), reply_parts("二答")]);
    let store = Arc::new(MemoryStore::new());
    let options = SessionOptions::new().with_workspace_root("/repo");
    let session = Session::create(store.clone(), agent(&model, Vec::new()), options);
    session.prompt_text("一").unwrap().outcome().await.unwrap();
    session.prompt_text("二").unwrap().outcome().await.unwrap();

    let forked = session
        .fork(3, agent(&model, Vec::new()), SessionOptions::new())
        .await
        .unwrap();
    assert_ne!(forked.id(), session.id());
    assert_eq!(
        forked.header().forked_from,
        Some(ForkOrigin {
            session: session.id().clone(),
            entry: 3
        })
    );
    assert_eq!(forked.header().workspace_root.as_deref(), Some("/repo"));
    let stored = store.load(forked.id()).await.unwrap();
    assert_eq!(stored.entries.len(), 3);
    assert_eq!(
        forked.context().await.unwrap(),
        [Message::user("一"), Message::assistant("一答")]
    );
    assert!(session
        .fork(99, agent(&model, Vec::new()), SessionOptions::new())
        .await
        .is_err());
}

/// 大文件以 blob 引用落盘，交给模型前还原
#[tokio::test]
async fn large_files_are_stored_as_blobs_and_restored() {
    let model = ScriptedModel::new(vec![reply_parts("看到了")]);
    let store = Arc::new(MemoryStore::new());
    let options = SessionOptions::new().with_blob_threshold(16);
    let session = Session::create(store.clone(), agent(&model, Vec::new()), options);
    let image = vec![7u8; 64];
    let message = Message::user_with_file("看图", "image/png", FileData::from_bytes(image.clone()));
    session
        .prompt(vec![message.clone()])
        .unwrap()
        .outcome()
        .await
        .unwrap();

    let stored = store.load(session.id()).await.unwrap();
    let EntryBody::Message(input) = &stored.entries[1].body else {
        panic!("第 2 条应为输入");
    };
    assert_eq!(input.origin, MessageOrigin::Input);
    let Message::User { content, .. } = &input.message else {
        panic!("应为用户消息");
    };
    let Some(UserPart::File {
        data: FileData::Url { url },
        ..
    }) = content.get(1)
    else {
        panic!("文件应以引用落盘: {content:?}");
    };
    assert!(url.starts_with("zach-blob:sha256-"));
    assert_eq!(session.context().await.unwrap()[0], message);
}

/// 写入失败后不再执行任何工具，运行以写入错误结束，会话拒绝后续运行
#[tokio::test]
async fn write_failures_stop_tools_and_the_session() {
    let model = ScriptedModel::new(vec![call_parts(&[("c1", "touch", json!({}))])]);
    let (executed, touch) = fn_tool_flag("touch");
    let store = Arc::new(FailingStore::new(1));
    let session = Session::create(store, agent(&model, vec![touch]), SessionOptions::new());

    let result = session.prompt_text("动手").unwrap().outcome().await;
    assert!(matches!(result, Err(SessionError::Persist(reason)) if reason.contains("磁盘已满")));
    assert!(!executed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(session.failure().is_some());
    assert!(matches!(
        session.prompt_text("再来"),
        Err(SessionError::Persist(_))
    ));
}

#[tokio::test]
async fn a_second_run_is_rejected_while_one_is_in_progress() {
    let model = ScriptedModel::new(Vec::new());
    let session = Session::create(
        Arc::new(MemoryStore::new()),
        agent(&model, Vec::new()),
        SessionOptions::new(),
    );
    let mut run = session.prompt_text("一").unwrap();
    assert!(matches!(session.prompt_text("二"), Err(SessionError::Busy)));

    // 等运行真正开始再中止
    while let Some(event) = run.next().await {
        if matches!(
            event,
            SessionEvent::Agent(zach_agent::AgentEvent::StepStart { .. })
        ) {
            session.agent().abort();
        }
    }
    assert!(run.outcome().await.unwrap().aborted);
    assert!(!session.is_running());
}
