//! 确定性验证事件发送的背压、取消及独立终止事件

use super::*;
use crate::agent::Agent;
use futures::poll;
use tokio::time::timeout;
use zach_ai_core::{CallOptions, GenerateResult, LanguageModel, LanguageModelStream, ModelError};

const WAIT: Duration = Duration::from_secs(2);

/// 仅用于构造宿主共享状态，不参与运行。
struct UnusedModel;

#[async_trait]
impl LanguageModel for UnusedModel {
    fn provider(&self) -> &str {
        "test"
    }

    fn model_id(&self) -> &str {
        "unused"
    }

    async fn do_generate(&self, _: CallOptions) -> Result<GenerateResult, ModelError> {
        unreachable!("宿主单元测试不调用模型")
    }

    async fn do_stream(&self, _: CallOptions) -> Result<LanguageModelStream, ModelError> {
        unreachable!("宿主单元测试不调用模型")
    }
}

fn host() -> (
    RunHost,
    mpsc::Receiver<AgentEvent>,
    mpsc::Receiver<AgentEvent>,
) {
    let inner = Agent::new(Arc::new(UnusedModel)).inner;
    let approval = inner.lock().approval.clone();
    let (events, receiver) = mpsc::channel(1);
    let (terminal, terminal_receiver) = mpsc::channel(1);
    (
        RunHost::new(
            inner,
            events,
            terminal,
            CancellationToken::new(),
            approval,
            false,
        ),
        receiver,
        terminal_receiver,
    )
}

#[tokio::test]
async fn cancellation_releases_an_already_blocked_send_and_cleanup_sends() {
    let (host, mut events, mut terminal) = host();
    host.emit(AgentEvent::StepFinish { step_index: 0 }).await;
    {
        let sending = host.emit(AgentEvent::StepFinish { step_index: 1 });
        tokio::pin!(sending);
        // 槽位已满，明确确认发送正在等待，而不是依赖调度或休眠猜测。
        assert!(poll!(sending.as_mut()).is_pending());
        host.cancel.cancel();
        timeout(WAIT, sending).await.expect("取消应打断发送等待");
    }
    timeout(WAIT, host.emit(AgentEvent::StepFinish { step_index: 2 }))
        .await
        .expect("取消后的收尾也不能再次等待槽位");
    host.emit(AgentEvent::RunAbort { reason: None }).await;
    drop(host);

    assert!(matches!(
        events.recv().await,
        Some(AgentEvent::StepFinish { step_index: 0 })
    ));
    assert!(events.recv().await.is_none());
    assert!(matches!(
        terminal.recv().await,
        Some(AgentEvent::RunAbort { .. })
    ));
    assert!(terminal.recv().await.is_none());
}

#[tokio::test]
async fn uncancelled_send_keeps_backpressure_and_does_not_drop_events() {
    let (host, mut events, _) = host();
    host.emit(AgentEvent::StepFinish { step_index: 0 }).await;
    let sending = host.emit(AgentEvent::StepFinish { step_index: 1 });
    tokio::pin!(sending);
    assert!(poll!(sending.as_mut()).is_pending());
    assert!(matches!(
        events.recv().await,
        Some(AgentEvent::StepFinish { step_index: 0 })
    ));
    timeout(WAIT, sending).await.expect("腾出槽位后应恢复发送");
    assert!(matches!(
        events.recv().await,
        Some(AgentEvent::StepFinish { step_index: 1 })
    ));
}

#[tokio::test]
async fn every_terminal_event_bypasses_a_full_queue_without_cancellation() {
    for event in [
        AgentEvent::RunAbort { reason: None },
        AgentEvent::RunError {
            error_text: "运行失败".into(),
        },
        AgentEvent::RunFinish {
            finish_reason: None,
            message_metadata: None,
        },
    ] {
        let (host, mut events, mut terminal) = host();
        host.emit(AgentEvent::StepFinish { step_index: 0 }).await;
        timeout(WAIT, host.emit(event.clone()))
            .await
            .expect("最终事件不能受普通队列背压影响");
        if matches!(event, AgentEvent::RunError { .. }) {
            assert_eq!(host.inner.lock().last_error.as_deref(), Some("运行失败"));
        }
        assert_eq!(terminal.recv().await, Some(event));
        assert!(matches!(
            events.recv().await,
            Some(AgentEvent::StepFinish { step_index: 0 })
        ));
    }
}

#[tokio::test]
async fn cancellation_preserves_ordinary_events_when_space_is_available() {
    let (host, mut events, _) = host();
    host.cancel.cancel();
    host.emit(AgentEvent::StepFinish { step_index: 0 }).await;
    assert!(matches!(
        events.recv().await,
        Some(AgentEvent::StepFinish { step_index: 0 })
    ));
}

#[tokio::test]
async fn dropped_consumer_does_not_block_ordinary_or_terminal_events() {
    let (host, events, terminal) = host();
    drop(events);
    drop(terminal);
    timeout(WAIT, async {
        host.emit(AgentEvent::StepFinish { step_index: 0 }).await;
        host.emit(AgentEvent::RunAbort { reason: None }).await;
    })
    .await
    .expect("消费者离开后发送不应阻塞");
}
