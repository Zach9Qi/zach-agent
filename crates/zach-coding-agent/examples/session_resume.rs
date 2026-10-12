//! 示例：会话的创建、恢复、回退与压缩
//!
//! 运行：`cargo run -p zach-coding-agent --example session_resume`
//!
//! 本示例不访问网络。[`EchoModel`] 复述最后一个问题并报出它看到了几个问题，
//! 用来观察恢复与回退后模型实际收到的上下文；[`JoinSummarizer`] 把被压缩的问题拼成摘要，
//! 代替真实的摘要模型。会话保存在进程内的 `MemoryStore` 中。

use async_trait::async_trait;
use futures::StreamExt;
use std::sync::Arc;
use zach_agent::{Agent, AgentEvent};
use zach_ai_core::{
    CallOptions, FinishReason, GenerateResult, LanguageModel, LanguageModelStream, Message,
    ModelError, StreamPart, Usage, UserPart,
};
use zach_coding_agent::session::{
    CompactionOptions, EntryBody, MemoryStore, Session, SessionEvent, SessionOptions, SessionStore,
    Summarizer, SummaryRequest,
};

/// 复述最后一个问题，并报出上下文中有几个问题
struct EchoModel;

#[async_trait]
impl LanguageModel for EchoModel {
    fn provider(&self) -> &str {
        "example"
    }

    fn model_id(&self) -> &str {
        "echo"
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        Err(ModelError::unsupported("generate", None))
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let questions: Vec<String> = options.prompt.messages.iter().filter_map(text_of).collect();
        let reply = format!(
            "（我看到 {} 条用户消息）你刚才说：{}",
            questions.len(),
            questions.last().cloned().unwrap_or_default()
        );
        let parts = vec![
            StreamPart::TextStart {
                id: "t".into(),
                provider_metadata: None,
            },
            StreamPart::TextDelta {
                id: "t".into(),
                delta: reply,
                provider_metadata: None,
            },
            StreamPart::TextEnd {
                id: "t".into(),
                provider_metadata: None,
            },
            StreamPart::Finish {
                usage: Usage::simple(50, 10),
                finish_reason: FinishReason::stop(),
                provider_metadata: None,
            },
        ];
        Ok(Box::pin(futures::stream::iter(parts.into_iter().map(Ok))))
    }
}

/// 把被压缩的用户消息拼成摘要
struct JoinSummarizer;

#[async_trait]
impl Summarizer for JoinSummarizer {
    async fn summarize(&self, request: SummaryRequest<'_>) -> Result<String, ModelError> {
        let asked: Vec<String> = request.messages.iter().filter_map(text_of).collect();
        Ok(format!("用户此前问过：{}", asked.join("；")))
    }
}

fn text_of(message: &Message) -> Option<String> {
    let Message::User { content, .. } = message else {
        return None;
    };
    content.iter().find_map(|part| match part {
        UserPart::Text { text, .. } => Some(text.clone()),
        UserPart::File { .. } => None,
    })
}

fn agent() -> Agent {
    Agent::builder(Arc::new(EchoModel))
        .system_prompt("你是复读助手")
        .build()
}

/// 发起一次提问，打印模型回复
async fn ask(session: &Session, text: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("[用户] {text}");
    let mut run = session.prompt_text(text)?;
    while let Some(event) = run.next().await {
        match event {
            SessionEvent::Agent(AgentEvent::TextDelta { delta, .. }) => println!("[模型] {delta}"),
            SessionEvent::Compaction(event) => println!("[压缩] {event:?}"),
            _ => {}
        }
    }
    run.outcome().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryStore::new());
    let options = SessionOptions::new()
        .with_workspace_root("/repo")
        .with_compaction(
            CompactionOptions::new()
                .with_keep_recent_tokens(10)
                .with_summarizer(JoinSummarizer),
        );

    println!("== 新建会话");
    let session = Session::create(store.clone(), agent(), options.clone());
    ask(&session, "第一问").await?;
    ask(&session, "第二问").await?;

    println!("\n== 用新的 Agent 恢复会话，模型看得到之前的问题");
    let session = Session::open(store.clone(), session.id(), agent(), options).await?;
    ask(&session, "第三问").await?;

    println!("\n== 回退到第一问的回答之后，开出新分支；原分支仍保留在日志里");
    let first_answer = session.entries()[2].id;
    session.rewind(first_answer).await?;
    ask(&session, "换个问法").await?;

    println!("\n== 手动压缩，较早的消息被摘要替代");
    session.compact().await?;
    for message in session.context().await? {
        println!("  {message:?}");
    }

    println!("\n== 存储中的条目（只追加，从不改写）");
    for entry in store.load(session.id()).await?.entries {
        let kind = match &entry.body {
            EntryBody::Message(message) => format!("消息/{:?}", message.origin),
            EntryBody::Compaction(_) => "压缩".into(),
            EntryBody::Config(_) => "配置".into(),
            EntryBody::Custom(_) => "扩展".into(),
            EntryBody::Checkout { target } => format!("回退到 {target}"),
        };
        println!("  #{} 父 {:?}：{kind}", entry.id, entry.parent);
    }
    Ok(())
}
