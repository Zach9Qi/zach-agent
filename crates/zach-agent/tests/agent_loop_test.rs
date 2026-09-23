//! 低层循环：轮次调度、插队/追加、重试、中止与钩子

#[path = "support/mock.rs"]
mod mock;

use async_trait::async_trait;
use mock::{echo_tool, kinds, text, tool_calls, Script, ScriptedModel, TestHost};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use zach_agent::{
    continue_agent_loop, run_agent_loop, AgentContext, AgentError, AgentEvent, AgentHooks,
    LoopConfig, RequestState, RetryPolicy, RunOutput, SharedTool, TurnDecision, TurnInfo,
};
use zach_ai_core::{Message, ModelError, StreamPart, ToolPart, UnifiedFinishReason};

fn config(model: Arc<ScriptedModel>) -> LoopConfig {
    LoopConfig::new(model).with_retry(RetryPolicy {
        max_retries: 2,
        initial_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
        multiplier: 1.0,
    })
}

fn context(tools: Vec<SharedTool>) -> AgentContext {
    AgentContext {
        system_prompt: Some("你是助手".into()),
        tools,
        ..Default::default()
    }
}

async fn run(
    config: LoopConfig,
    context: AgentContext,
    host: &TestHost,
) -> Result<RunOutput, AgentError> {
    let prompts = vec![Message::user("你好")];
    run_agent_loop(prompts, context, config, host, CancellationToken::new()).await
}

#[tokio::test]
async fn text_reply_emits_full_lifecycle() {
    let model = ScriptedModel::new(vec![Script::Parts(text("你好呀"))]);
    let host = TestHost::default();
    let output = run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap();

    assert_eq!(
        host.kinds(),
        [
            "run-start",
            "step-start",
            "text-start",
            "text-delta",
            "text-finish",
            "step-finish",
            "run-finish"
        ]
    );
    assert_eq!(
        output.messages,
        vec![Message::user("你好"), Message::assistant("你好呀")]
    );
    assert_eq!(*host.messages.lock().unwrap(), output.messages);
    assert_eq!(output.steps, 1);
    assert!(!output.aborted);
    assert_eq!(output.usage.input_tokens.total, Some(10));

    let call = model.call(0);
    assert_eq!(call.prompt.messages[0], Message::system("你是助手"));
    assert!(call.tools.is_none());
}

#[tokio::test]
async fn tool_results_feed_the_next_turn() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "echo", r#"{"x":1}"#)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("完成")),
    ]);
    let host = TestHost::default();
    let output = run(
        config(model.clone()),
        context(vec![echo_tool("echo")]),
        &host,
    )
    .await
    .unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(output.steps, 2);
    assert_eq!(output.messages.len(), 4);
    assert_eq!(
        output.messages[2],
        Message::tool(vec![ToolPart::result_json(
            "c1",
            "echo",
            serde_json::json!({ "x": 1 })
        )])
    );
    assert_eq!(
        model.call(1).prompt.messages.last(),
        Some(&output.messages[2])
    );
    assert_eq!(model.call(0).tools.as_ref().map(Vec::len), Some(1));
    assert_eq!(output.usage.input_tokens.total, Some(20));

    let events = host.events();
    assert!(events.contains(&AgentEvent::StepStart { step_index: 1 }));
    let kinds = kinds(&events);
    let available = kinds
        .iter()
        .position(|k| k == "tool-input-available")
        .unwrap();
    let output_at = kinds
        .iter()
        .position(|k| k == "tool-output-available")
        .unwrap();
    assert!(available < output_at);
}

#[tokio::test]
async fn steering_messages_are_injected_between_turns() {
    let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
    let host = TestHost::default();
    host.steering
        .lock()
        .unwrap()
        .extend([vec![], vec![Message::user("插队")]]);
    let output = run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(output.messages[2], Message::user("插队"));
    assert_eq!(
        model.call(1).prompt.messages.last(),
        Some(&Message::user("插队"))
    );
}

#[tokio::test]
async fn follow_up_messages_continue_a_finished_run() {
    let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
    let host = TestHost::default();
    host.follow_up
        .lock()
        .unwrap()
        .push_back(vec![Message::user("追加")]);
    let output = run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(output.messages[2], Message::user("追加"));
}

#[tokio::test]
async fn retryable_failures_emit_step_retry_and_recover() {
    let model = ScriptedModel::new(vec![
        Script::Fail(ModelError::RateLimit("慢点".into())),
        Script::Items(vec![
            Ok(StreamPart::TextDelta {
                id: "t".into(),
                delta: "半".into(),
                provider_metadata: None,
            }),
            Err(ModelError::stream_error(
                "断开",
                std::io::Error::other("reset"),
            )),
        ]),
        Script::Parts(text("好")),
    ]);
    let host = TestHost::default();
    let output = run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap();

    assert_eq!(model.call_count(), 3);
    assert_eq!(
        host.kinds().iter().filter(|k| *k == "step-retry").count(),
        2
    );
    assert_eq!(output.messages.last(), Some(&Message::assistant("好")));
}

#[tokio::test]
async fn truncated_stream_is_retried() {
    let unfinished = text("丢失")[..3].to_vec();
    let model = ScriptedModel::new(vec![Script::Parts(unfinished), Script::Parts(text("好"))]);
    let host = TestHost::default();
    run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap();
    assert_eq!(model.call_count(), 2);
}

#[tokio::test]
async fn fatal_failures_emit_run_error_without_committing_the_step() {
    let model = ScriptedModel::new(vec![Script::Fail(ModelError::Authentication(
        "无效".into(),
    ))]);
    let host = TestHost::default();
    let error = run(config(model.clone()), context(vec![]), &host)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::Model(ModelError::Authentication(_))
    ));
    assert_eq!(host.kinds().last().map(String::as_str), Some("run-error"));
    assert_eq!(*host.messages.lock().unwrap(), vec![Message::user("你好")]);

    let model = ScriptedModel::new(
        (0..3)
            .map(|_| Script::Fail(ModelError::RateLimit("忙".into())))
            .collect(),
    );
    let host = TestHost::default();
    assert!(run(config(model.clone()), context(vec![]), &host)
        .await
        .is_err());
    assert_eq!(model.call_count(), 3);
}

#[tokio::test]
async fn abort_mid_stream_keeps_generated_text() {
    let model = ScriptedModel::new(vec![Script::Hang(text("部分")[..2].to_vec())]);
    let host = TestHost::default();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        trigger.cancel();
    });
    let prompts = vec![Message::user("你好")];
    let output = run_agent_loop(prompts, context(vec![]), config(model), &host, cancel)
        .await
        .unwrap();

    assert!(output.aborted);
    assert_eq!(output.messages.last(), Some(&Message::assistant("部分")));
    assert_eq!(
        host.kinds()[host.kinds().len() - 2..],
        ["step-finish", "run-abort"]
    );
}

struct Scheduling {
    decision: TurnDecision,
}

#[async_trait]
impl AgentHooks for Scheduling {
    async fn transform_context(&self, messages: Vec<Message>) -> Vec<Message> {
        let mut messages = messages;
        messages.insert(0, Message::user("注入的背景"));
        messages
    }

    async fn prepare_request(&self, request: &mut RequestState) {
        request.options.temperature = Some(0.2);
    }

    async fn finish_turn(&self, turn: &TurnInfo, _context: &AgentContext) -> TurnDecision {
        if turn.step_index == 0 {
            self.decision
        } else {
            TurnDecision::Default
        }
    }
}

#[tokio::test]
async fn finish_turn_can_end_or_force_another_turn() {
    let calls = Script::Parts(tool_calls(
        &[("c1", "echo", "{}")],
        UnifiedFinishReason::ToolCalls,
    ));
    let model = ScriptedModel::new(vec![calls]);
    let hooks = Arc::new(Scheduling {
        decision: TurnDecision::End,
    });
    let host = TestHost::default();
    let output = run(
        config(model.clone()).with_hooks(hooks),
        context(vec![echo_tool("echo")]),
        &host,
    )
    .await
    .unwrap();
    assert_eq!(model.call_count(), 1);
    assert_eq!(output.messages.len(), 3);

    let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
    let hooks = Arc::new(Scheduling {
        decision: TurnDecision::Continue,
    });
    let output = run(
        config(model.clone()).with_hooks(hooks),
        context(vec![]),
        &host,
    )
    .await
    .unwrap();
    assert_eq!(model.call_count(), 2);
    assert_eq!(output.steps, 2);

    let call = model.call(0);
    assert_eq!(call.temperature, Some(0.2));
    assert_eq!(call.prompt.messages[1], Message::user("注入的背景"));
    assert_eq!(output.messages[0], Message::user("你好"));
}

#[tokio::test]
async fn continue_requires_a_non_assistant_tail() {
    let host = TestHost::default();
    let model = ScriptedModel::new(vec![Script::Parts(text("继续"))]);
    let cancel = CancellationToken::new();

    let empty = continue_agent_loop(
        context(vec![]),
        config(model.clone()),
        &host,
        cancel.clone(),
    )
    .await;
    assert!(matches!(empty, Err(AgentError::NoMessages)));

    let mut tail_assistant = context(vec![]);
    tail_assistant.messages = vec![Message::user("问"), Message::assistant("答")];
    let result =
        continue_agent_loop(tail_assistant, config(model.clone()), &host, cancel.clone()).await;
    assert!(matches!(
        result,
        Err(AgentError::CannotContinueFromAssistant)
    ));

    let mut tail_user = context(vec![]);
    tail_user.messages = vec![Message::user("问")];
    let output = continue_agent_loop(tail_user, config(model.clone()), &host, cancel)
        .await
        .unwrap();
    assert_eq!(output.messages, vec![Message::assistant("继续")]);
}
