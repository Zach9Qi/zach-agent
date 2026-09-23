//! 有状态 Agent：状态同步、忙碌保护、中止、队列、审批与继续

#[path = "support/mock.rs"]
mod mock;

use futures::StreamExt;
use mock::{fn_tool, text, tool_calls, Script, ScriptedModel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zach_agent::{
    Agent, AgentError, AgentEvent, ApprovalDecision, QueueMode, RetryPolicy, ToolOutcome,
};
use zach_ai_core::{Message, ModelError, UnifiedFinishReason};

fn agent(model: Arc<ScriptedModel>) -> Agent {
    Agent::builder(model)
        .system_prompt("你是助手")
        .retry(RetryPolicy::none())
        .build()
}

#[tokio::test]
async fn transcript_accumulates_across_runs() {
    let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
    let agent = agent(model.clone());

    let (events, output) = agent.prompt_text("第一问").unwrap().collect().await;
    assert_eq!(
        events
            .first()
            .map(|e| matches!(e, AgentEvent::RunStart { .. })),
        Some(true)
    );
    assert_eq!(output.unwrap().messages.len(), 2);
    assert!(!agent.is_running());

    agent
        .prompt_text("第二问")
        .unwrap()
        .outcome()
        .await
        .unwrap();
    assert_eq!(
        agent.messages(),
        vec![
            Message::user("第一问"),
            Message::assistant("一"),
            Message::user("第二问"),
            Message::assistant("二"),
        ]
    );
    assert_eq!(model.call(1).prompt.messages.len(), 4);
}

#[tokio::test]
async fn busy_agent_rejects_new_runs_until_aborted() {
    let model = ScriptedModel::new(vec![Script::Hang(text("半")[..2].to_vec())]);
    let agent = agent(model);
    let run = agent.prompt_text("你好").unwrap();

    assert!(agent.is_running());
    assert!(matches!(agent.prompt_text("再来"), Err(AgentError::Busy)));
    assert!(matches!(agent.continue_run(), Err(AgentError::Busy)));
    assert!(matches!(agent.reset(), Err(AgentError::Busy)));

    tokio::time::sleep(Duration::from_millis(20)).await;
    agent.abort();
    let output = run.outcome().await.unwrap();
    agent.wait_for_idle().await;
    assert!(output.aborted);
    assert!(!agent.is_running());
    assert_eq!(agent.messages().last(), Some(&Message::assistant("半")));

    agent.reset().unwrap();
    assert!(agent.messages().is_empty());
}

#[tokio::test]
async fn dropping_the_run_handle_does_not_abort() {
    let model = ScriptedModel::new(vec![Script::Parts(text("完成"))]);
    let agent = agent(model);
    drop(agent.prompt_text("你好").unwrap());
    agent.wait_for_idle().await;
    assert_eq!(agent.messages().last(), Some(&Message::assistant("完成")));
}

#[tokio::test]
async fn steering_from_a_running_tool_reaches_the_next_request() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "probe", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("好")),
    ]);
    let agent = agent(model.clone());
    let seen_pending = Arc::new(Mutex::new(Vec::new()));
    let (handle, seen) = (agent.clone(), seen_pending.clone());
    agent.set_tools(vec![fn_tool("probe", move |_, _| {
        *seen.lock().unwrap() = handle.pending_tool_calls();
        handle.steer(Message::user("顺便看看这个"));
        async { Ok(ToolOutcome::text("ok")) }
    })
    .shared()]);

    agent.prompt_text("开始").unwrap().outcome().await.unwrap();
    assert_eq!(*seen_pending.lock().unwrap(), ["c1"]);
    assert!(agent.pending_tool_calls().is_empty());
    assert_eq!(
        model.call(1).prompt.messages.last(),
        Some(&Message::user("顺便看看这个"))
    );
}

#[tokio::test]
async fn queue_mode_controls_how_many_messages_each_injection_takes() {
    for (mode, expected_calls) in [(QueueMode::OneAtATime, 2), (QueueMode::All, 1)] {
        let model = ScriptedModel::new(vec![Script::Parts(text("一")), Script::Parts(text("二"))]);
        let agent = agent(model.clone());
        agent.set_steering_mode(mode);
        agent.steer(Message::user("甲"));
        agent.steer(Message::user("乙"));
        assert_eq!(
            agent.peek_queued_messages().len(),
            if mode == QueueMode::All { 2 } else { 1 }
        );

        agent.prompt_text("开始").unwrap().outcome().await.unwrap();
        assert_eq!(model.call_count(), expected_calls);
        assert!(!agent.has_queued_messages());
    }
}

#[tokio::test]
async fn approvals_are_answered_through_the_agent_handle() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "rm", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("删好了")),
    ]);
    let agent = agent(model);
    let tool = fn_tool("rm", |_, _| async { Ok(ToolOutcome::text("已删除")) });
    agent.set_tools(vec![tool.needs_approval().shared()]);
    assert!(matches!(
        agent.respond_approval("nope", ApprovalDecision::approve()),
        Err(AgentError::ApprovalNotFound(_))
    ));

    let mut run = agent.prompt_text("清理").unwrap();
    let mut executed = false;
    while let Some(event) = run.next().await {
        match event {
            AgentEvent::ToolApprovalRequest { approval_id, .. } => {
                assert_eq!(agent.pending_approvals(), std::slice::from_ref(&approval_id));
                agent
                    .respond_approval(&approval_id, ApprovalDecision::approve())
                    .unwrap();
            }
            AgentEvent::ToolOutputAvailable { .. } => executed = true,
            _ => {}
        }
    }
    assert!(executed);
    assert!(agent.pending_approvals().is_empty());
}

#[tokio::test]
async fn continue_run_retries_after_failure_and_drains_queues_after_replies() {
    let model = ScriptedModel::new(vec![
        Script::Fail(ModelError::Authentication("过期".into())),
        Script::Parts(text("恢复了")),
        Script::Parts(text("追加的回复")),
    ]);
    let agent = agent(model.clone());
    assert!(matches!(agent.continue_run(), Err(AgentError::NoMessages)));

    let failed = agent.prompt_text("你好").unwrap().outcome().await;
    assert!(failed.is_err());
    assert!(agent.last_error().unwrap().contains("鉴权失败"));
    assert_eq!(agent.messages(), vec![Message::user("你好")]);

    agent.continue_run().unwrap().outcome().await.unwrap();
    assert!(agent.last_error().is_none());
    assert_eq!(agent.messages().last(), Some(&Message::assistant("恢复了")));

    assert!(matches!(
        agent.continue_run(),
        Err(AgentError::CannotContinueFromAssistant)
    ));
    agent.follow_up(Message::user("再补充一点"));
    agent.continue_run().unwrap().outcome().await.unwrap();
    assert_eq!(model.call_count(), 3);
    assert_eq!(
        agent.messages().last(),
        Some(&Message::assistant("追加的回复"))
    );
}
