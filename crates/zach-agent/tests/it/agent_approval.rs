//! 有状态 Agent 的审批路径：审批处理器路由、超时，以及事件消费者离开时不得悬挂

use crate::support::{fn_tool, text, tool_calls, Script, ScriptedModel};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use zach_agent::{
    Agent, AgentBuilder, AgentEvent, ApprovalDecision, ApprovalRoute, ApproveAll, DenyAll,
    InteractiveApproval, RetryPolicy, ToolOutcome,
};
use zach_ai_core::{Message, ToolPart, ToolResultOutput, UnifiedFinishReason};

/// 一次运行：模型先调用需要审批的 `rm`，得到结果后回复文本。返回构造器以便叠加审批处理器。
fn builder_with_approval_tool() -> (AgentBuilder, Arc<AtomicBool>) {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "rm", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("处理完了")),
    ]);
    let executed = Arc::new(AtomicBool::new(false));
    let flag = executed.clone();
    let tool = fn_tool("rm", move |_, _| {
        let flag = flag.clone();
        async move {
            flag.store(true, Ordering::SeqCst);
            Ok(ToolOutcome::text("已删除"))
        }
    });
    let builder = Agent::builder(model)
        .retry(RetryPolicy::none())
        .tool(tool.needs_approval().shared());
    (builder, executed)
}

fn agent_with_approval_tool() -> Agent {
    builder_with_approval_tool().0.build()
}

/// 断言对话记录里 `c1` 的结果是"被拒绝"
fn assert_denied(agent: &Agent) {
    let denied = agent.messages().iter().any(|message| {
        matches!(
            message,
            Message::Tool { content, .. } if content.iter().any(|part| matches!(
                part,
                ToolPart::ToolResult { tool_call_id, output: ToolResultOutput::ExecutionDenied { .. }, .. }
                    if tool_call_id == "c1"
            ))
        )
    });
    assert!(denied, "c1 应被拒绝: {:?}", agent.messages());
}

const WAIT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn outcome_without_event_consumer_denies_pending_approval() {
    let agent = agent_with_approval_tool();

    let run = agent.prompt_text("清理").unwrap();
    let output = timeout(WAIT, run.outcome())
        .await
        .expect("outcome 不应悬挂")
        .unwrap();

    assert_eq!(
        output.messages.last(),
        Some(&Message::assistant("处理完了"))
    );
    assert_denied(&agent);
    assert!(agent.pending_approvals().is_empty());
    assert!(!agent.is_running());
}

#[tokio::test]
async fn dropping_run_with_pending_approval_releases_agent() {
    let agent = agent_with_approval_tool();

    drop(agent.prompt_text("清理").unwrap());
    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("丢弃 AgentRun 后 Agent 应能回到空闲");

    assert_denied(&agent);
    assert!(agent.pending_approvals().is_empty());
    // Agent 没有被"卡死"在忙碌状态，可以继续发起新的运行
    assert!(agent.reset().is_ok());
}

#[tokio::test]
async fn consumer_leaving_after_receiving_request_denies_approval() {
    let agent = agent_with_approval_tool();

    let mut run = agent.prompt_text("清理").unwrap();
    let mut saw_request = false;
    while let Some(event) = run.next().await {
        if matches!(event, AgentEvent::ToolApprovalRequest { .. }) {
            saw_request = true;
            break;
        }
    }
    assert!(saw_request);
    // 收到审批请求后消费者离开而不答复
    drop(run);

    timeout(WAIT, agent.wait_for_idle())
        .await
        .expect("消费者离开后运行应能结束");
    assert_denied(&agent);
    assert!(agent.pending_approvals().is_empty());
}

#[tokio::test]
async fn deny_all_handler_rejects_without_asking() {
    let (builder, executed) = builder_with_approval_tool();
    let agent = builder.approval(DenyAll).build();

    let (events, output) = timeout(WAIT, agent.prompt_text("清理").unwrap().collect())
        .await
        .unwrap();
    output.unwrap();

    assert!(!executed.load(Ordering::SeqCst));
    assert_denied(&agent);
    // 自动裁决不会向使用者呈现审批请求，只有裁决结果
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolApprovalRequest { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolApprovalResponse { approved: false, reason: Some(r), .. } if r.contains("审批策略")
    )));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolOutputDenied { .. })));
}

#[tokio::test]
async fn approve_all_handler_executes_without_asking() {
    let (builder, executed) = builder_with_approval_tool();
    let agent = builder.approval(ApproveAll).build();

    // 不消费事件也能跑完：处理器自动裁决，不依赖事件消费者
    timeout(WAIT, agent.prompt_text("清理").unwrap().outcome())
        .await
        .unwrap()
        .unwrap();
    assert!(executed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn closure_handler_can_mix_auto_decision_and_asking() {
    let (builder, executed) = builder_with_approval_tool();
    // 白名单策略：只有 `ls` 自动放行，其余转人工
    let agent = builder
        .approval(|request: &zach_agent::ApprovalRequest| {
            if request.tool_name == "ls" {
                ApprovalRoute::approve()
            } else {
                ApprovalRoute::ask()
            }
        })
        .build();

    let mut run = agent.prompt_text("清理").unwrap();
    let mut asked = false;
    while let Some(event) = timeout(WAIT, run.next()).await.unwrap() {
        if let AgentEvent::ToolApprovalRequest { approval_id, .. } = event {
            asked = true;
            agent
                .respond_approval(&approval_id, ApprovalDecision::approve())
                .unwrap();
        }
    }
    assert!(asked);
    assert!(executed.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn interactive_timeout_denies_unanswered_request() {
    let (builder, executed) = builder_with_approval_tool();
    let agent = builder
        .approval(InteractiveApproval::with_timeout(Duration::from_millis(50)))
        .build();

    // 消费事件但从不答复
    let (events, output) = timeout(WAIT, agent.prompt_text("清理").unwrap().collect())
        .await
        .expect("超时后应自动拒绝而不是悬挂");
    output.unwrap();

    assert!(!executed.load(Ordering::SeqCst));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolApprovalResponse { approved: false, reason: Some(r), .. } if r.contains("超时")
    )));
    assert!(agent.pending_approvals().is_empty());
}

#[tokio::test]
async fn approval_handler_can_be_replaced_between_runs() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "rm", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("第一次")),
        Script::Parts(tool_calls(
            &[("c2", "rm", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("第二次")),
    ]);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    let tool = fn_tool("rm", move |_, _| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutcome::text("已删除"))
        }
    });
    let agent = Agent::builder(model)
        .retry(RetryPolicy::none())
        .tool(tool.needs_approval().shared())
        .approval(DenyAll)
        .build();

    timeout(WAIT, agent.prompt_text("一").unwrap().outcome())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    agent.set_approval_handler(ApproveAll);
    timeout(WAIT, agent.prompt_text("二").unwrap().outcome())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
