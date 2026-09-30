//! 有界事件流背压：消费者保留句柄但停止消费时，取消仍须完成并保留唯一终止事件。

use crate::support::{finish, fn_tool, tool_calls, Script, ScriptedModel};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::timeout;
use zach_agent::{
    Agent, AgentContext, AgentEvent, AgentHooks, AgentRun, RetryPolicy, ToolOutcome, TurnDecision,
    TurnInfo,
};
use zach_ai_core::{
    AssistantPart, CallOptions, GenerateResult, LanguageModel, LanguageModelStream, Message,
    ModelError, StreamPart, ToolPart, ToolResultOutput, UnifiedFinishReason,
};

const WAIT: Duration = Duration::from_secs(3);
// 与 Agent 内部事件容量对应；容量变化时应同步调整背压屏障。
const EVENT_BUFFER: usize = 256;
const DELTAS: usize = 4096;

/// 流在即将被普通事件队列阻塞的分块上通知测试，不依赖时间估计。
struct FloodModel {
    polled: Arc<AtomicUsize>,
    full: Arc<Notify>,
}

impl FloodModel {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            polled: Arc::new(AtomicUsize::new(0)),
            full: Arc::new(Notify::new()),
        })
    }
}

#[async_trait]
impl LanguageModel for FloodModel {
    fn provider(&self) -> &str {
        "mock"
    }

    fn model_id(&self) -> &str {
        "backpressure"
    }

    async fn do_generate(&self, _: CallOptions) -> Result<GenerateResult, ModelError> {
        Err(ModelError::unsupported("generate", None))
    }

    async fn do_stream(&self, _: CallOptions) -> Result<LanguageModelStream, ModelError> {
        Ok(Box::pin(FloodStream {
            index: 0,
            polled: self.polled.clone(),
            full: self.full.clone(),
        }))
    }
}

struct FloodStream {
    index: usize,
    polled: Arc<AtomicUsize>,
    full: Arc<Notify>,
}

impl Stream for FloodStream {
    type Item = Result<StreamPart, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let index = self.index;
        self.index += 1;
        let part = match index {
            0 => StreamPart::TextStart {
                id: "t".into(),
                provider_metadata: None,
            },
            1..=DELTAS => {
                self.polled.store(index, Ordering::SeqCst);
                // RunStart、StepStart、TextStart 加前 253 个 delta 已占满队列。
                if index == EVENT_BUFFER - 2 {
                    self.full.notify_one();
                }
                StreamPart::TextDelta {
                    id: "t".into(),
                    delta: format!("{index},"),
                    provider_metadata: None,
                }
            }
            n if n == DELTAS + 1 => StreamPart::TextEnd {
                id: "t".into(),
                provider_metadata: None,
            },
            n if n == DELTAS + 2 => finish(UnifiedFinishReason::Stop),
            _ => return Poll::Ready(None),
        };
        Poll::Ready(Some(Ok(part)))
    }
}

fn assert_terminal(events: &[AgentEvent], aborted: bool) {
    let terminals = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::RunAbort { .. }
                    | AgentEvent::RunFinish { .. }
                    | AgentEvent::RunError { .. }
            )
        })
        .count();
    assert_eq!(terminals, 1, "每轮必须恰好有一个终止事件");
    assert!(if aborted {
        matches!(events.last(), Some(AgentEvent::RunAbort { .. }))
    } else {
        matches!(events.last(), Some(AgentEvent::RunFinish { .. }))
    });
}

/// 必须先证明运行已空闲，再消费；提前 outcome、drop 或 collect 会解除旧实现的死锁。
async fn assert_aborted_after_idle(agent: &Agent, run: AgentRun) -> Vec<AgentEvent> {
    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("保留未消费的 AgentRun 时，取消必须解除事件发送的背压");
    assert!(!agent.is_running());
    assert!(agent.pending_tool_calls().is_empty());
    assert!(agent.pending_approvals().is_empty());
    let (events, output) = timeout(WAIT, run.collect()).await.unwrap();
    assert!(output.unwrap().aborted);
    assert_terminal(&events, true);
    events
}

async fn cancel_text_flood(abort_via_agent: bool) {
    let model = FloodModel::new();
    let agent = Agent::builder(model.clone())
        .retry(RetryPolicy::none())
        .build();
    let run = agent.prompt_text("大量输出").unwrap();
    timeout(WAIT, model.full.notified()).await.unwrap();
    // 单线程运行时中，通知后生产者继续运行至 send/reserve 阻塞才让出执行权。
    assert_eq!(model.polled.load(Ordering::SeqCst), EVENT_BUFFER - 2);
    assert!(agent.is_running());
    if abort_via_agent {
        agent.abort();
    } else {
        run.abort();
    }
    let events = assert_aborted_after_idle(&agent, run).await;
    assert_eq!(events.len(), EVENT_BUFFER + 1);
}

#[tokio::test(flavor = "current_thread")]
async fn run_abort_releases_full_text_queue_without_consuming() {
    cancel_text_flood(false).await;
}

#[tokio::test(flavor = "current_thread")]
async fn agent_abort_releases_full_text_queue_without_consuming() {
    cancel_text_flood(true).await;
}

#[tokio::test(flavor = "current_thread")]
async fn consuming_large_stream_preserves_every_delta_and_final_event() {
    let agent = Agent::new(FloodModel::new());
    let (events, output) = timeout(WAIT, agent.prompt_text("完整输出").unwrap().collect())
        .await
        .unwrap();
    assert!(!output.unwrap().aborted);
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { delta, .. } => Some(delta.clone()),
            _ => None,
        })
        .collect();
    let expected: Vec<_> = (1..=DELTAS).map(|n| format!("{n},")).collect();
    assert_eq!(deltas, expected, "正常消费不能丢失或重排增量");
    assert_eq!(
        agent.messages().last(),
        Some(&Message::assistant(expected.concat()))
    );
    assert_terminal(&events, false);
}

fn assert_tool_pairing(agent: &Agent) {
    let mut calls = Vec::new();
    let mut results = Vec::new();
    for message in agent.messages() {
        match message {
            Message::Assistant { content, .. } => {
                for part in content {
                    if let AssistantPart::ToolCall { tool_call_id, .. } = part {
                        calls.push(tool_call_id);
                    }
                }
            }
            Message::Tool { content, .. } => {
                for part in content {
                    if let ToolPart::ToolResult {
                        tool_call_id,
                        output,
                        ..
                    } = part
                    {
                        assert!(matches!(output, ToolResultOutput::ErrorText { .. }));
                        results.push(tool_call_id);
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(calls, ["c1"], "历史必须保留原始调用");
    assert_eq!(results, calls, "取消后每个工具调用仍须有且仅有一个结果");
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_full_tool_progress_queue_repairs_history_and_pending_state() {
    let started = Arc::new(Notify::new());
    let signal = started.clone();
    let tool = fn_tool("progress", move |_, ctx| {
        let signal = signal.clone();
        async move {
            for _ in 0..DELTAS {
                ctx.report_progress(ToolResultOutput::text("进度"));
            }
            signal.notify_one();
            std::future::pending::<Result<ToolOutcome, zach_agent::ToolError>>().await
        }
    });
    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &[("c1", "progress", "{}")],
        UnifiedFinishReason::ToolCalls,
    ))]);
    let agent = Agent::builder(model).tool(tool.shared()).build();
    let run = agent.prompt_text("执行工具").unwrap();
    timeout(WAIT, started.notified()).await.unwrap();
    // 进度已同步排队；允许生产者越过 Tokio 协作预算的让步，直到有界发送阻塞。
    // 不使用 sleep；下方精确检查收集数量，保证取消时确实填满了普通队列。
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    assert_eq!(agent.pending_tool_calls(), ["c1"]);
    agent.abort();
    let events = assert_aborted_after_idle(&agent, run).await;
    assert_eq!(events.len(), EVENT_BUFFER + 1);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolOutputAvailable {
            preliminary: true,
            ..
        }
    )));
    assert_tool_pairing(&agent);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_pending_approval_clears_approval_and_tool_state() {
    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &[("c1", "approval", "{}")],
        UnifiedFinishReason::ToolCalls,
    ))]);
    let tool = fn_tool("approval", |_, _| async {
        panic!("等待审批时取消，不应执行工具")
    });
    let agent = Agent::builder(model)
        .tool(tool.needs_approval().shared())
        .build();
    let mut run = agent.prompt_text("等待审批").unwrap();
    let mut prefix = Vec::new();
    let approval_id = timeout(WAIT, async {
        loop {
            let event = run.next().await.expect("应先收到审批请求");
            let id = match &event {
                AgentEvent::ToolApprovalRequest { approval_id, .. } => Some(approval_id.clone()),
                _ => None,
            };
            prefix.push(event);
            if let Some(id) = id {
                break id;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(agent.pending_approvals(), [approval_id]);
    assert_eq!(agent.pending_tool_calls(), ["c1"]);
    run.abort();
    prefix.extend(assert_aborted_after_idle(&agent, run).await);
    assert_terminal(&prefix, true);
    assert_tool_pairing(&agent);
}

/// 通知时助手消息已经提交，随后 StepFinish 会遭遇满队列背压。
struct FinishTurnBarrier(Arc<Notify>);

#[async_trait]
impl AgentHooks for FinishTurnBarrier {
    async fn finish_turn(&self, _: &TurnInfo, _: &AgentContext) -> TurnDecision {
        self.0.notify_one();
        TurnDecision::Default
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelling_blocked_final_step_finish_returns_abort_not_finish() {
    let barrier = Arc::new(Notify::new());
    // 3 个起始事件 + 252 个增量 + TextFinish 恰好占满 256 个普通槽。
    let count = EVENT_BUFFER - 4;
    let mut parts = vec![StreamPart::TextStart {
        id: "t".into(),
        provider_metadata: None,
    }];
    parts.extend((0..count).map(|_| StreamPart::TextDelta {
        id: "t".into(),
        delta: "界".into(),
        provider_metadata: None,
    }));
    parts.push(StreamPart::TextEnd {
        id: "t".into(),
        provider_metadata: None,
    });
    parts.push(finish(UnifiedFinishReason::Stop));
    let agent = Agent::builder(ScriptedModel::new(vec![Script::Parts(parts)]))
        .hooks(FinishTurnBarrier(barrier.clone()))
        .build();
    let run = agent.prompt_text("在最终步骤事件处取消").unwrap();
    timeout(WAIT, barrier.notified()).await.unwrap();
    assert!(agent.is_running());
    assert_eq!(
        agent.messages().last(),
        Some(&Message::assistant("界".repeat(count)))
    );
    // 保留句柄、不消费，必须先确认取消解除 StepFinish 的发送阻塞。
    run.abort();
    let events = assert_aborted_after_idle(&agent, run).await;
    assert_eq!(events.len(), EVENT_BUFFER + 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::TextDelta { .. }))
            .count(),
        count
    );
    assert!(matches!(
        events[EVENT_BUFFER - 1],
        AgentEvent::TextFinish { .. }
    ));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::StepFinish { .. })));
    assert_eq!(
        agent.messages().last(),
        Some(&Message::assistant("界".repeat(count)))
    );
}
