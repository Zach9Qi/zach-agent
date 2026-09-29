//! 有状态 Agent 的审批路径：事件消费者离开时不得悬挂

#[path = "support/mock.rs"]
mod mock;

use futures::StreamExt;
use mock::{fn_tool, text, tool_calls, Script, ScriptedModel};
use std::time::Duration;
use tokio::time::timeout;
use zach_agent::{Agent, AgentEvent, RetryPolicy, ToolOutcome};
use zach_ai_core::{Message, ToolPart, ToolResultOutput, UnifiedFinishReason};

/// 一次运行：模型先调用需要审批的 `rm`，得到结果后回复文本
fn agent_with_approval_tool() -> Agent {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "rm", "{}")],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("处理完了")),
    ]);
    let tool = fn_tool("rm", |_, _| async { Ok(ToolOutcome::text("已删除")) });
    Agent::builder(model)
        .retry(RetryPolicy::none())
        .tool(tool.needs_approval().shared())
        .build()
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
