//! 钩子放行/拒绝/改写，以及本地审批与厂商审批

use super::{config, executed_flag, one_call, run_with, tool_output};
use crate::support::{echo_tool, finish, fn_tool, text, Script, ScriptedModel, TestHost};
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use zach_agent::{
    AgentEvent, AgentHooks, ApprovalDecision, ToolCallDecision, ToolCallInfo, ToolOutcome,
};
use zach_ai_core::{Message, StreamPart, ToolPart, ToolResultOutput, UnifiedFinishReason};

/// 固定返回某个前置决策，并给文本结果追加审计后缀的钩子
struct Policy(ToolCallDecision);

#[async_trait]
impl AgentHooks for Policy {
    async fn before_tool_call(&self, _call: &ToolCallInfo<'_>) -> ToolCallDecision {
        self.0.clone()
    }

    async fn after_tool_call(&self, _call: &ToolCallInfo<'_>, outcome: ToolOutcome) -> ToolOutcome {
        match outcome.output {
            ToolResultOutput::Text { value, .. } => ToolOutcome::text(format!("{value}（已审计）")),
            _ => outcome,
        }
    }
}

#[tokio::test]
async fn before_hook_can_deny_and_after_hook_can_rewrite() {
    let host = TestHost::default();
    let hooks = Arc::new(Policy(ToolCallDecision::deny("禁止")));
    let output = run_with(
        config(one_call("echo", "{}")).with_hooks(hooks),
        vec![echo_tool("echo")],
        &host,
    )
    .await;
    assert!(host.events().contains(&AgentEvent::ToolOutputDenied {
        tool_call_id: "c1".into(),
        reason: Some("禁止".into())
    }));
    assert_eq!(tool_output(&output), ToolResultOutput::denied(Some("禁止")));

    let host = TestHost::default();
    let tool = fn_tool("say", |_, _| async { Ok(ToolOutcome::text("hi")) }).shared();
    let hooks = Arc::new(Policy(ToolCallDecision::Allow));
    let output = run_with(
        config(one_call("say", "{}")).with_hooks(hooks),
        vec![tool],
        &host,
    )
    .await;
    assert_eq!(tool_output(&output), ToolResultOutput::text("hi（已审计）"));
}

#[tokio::test]
async fn approval_is_requested_and_honored() {
    let (executed, tool) = executed_flag();
    let host = TestHost::default();
    run_with(config(one_call("guarded", r#"{"p":1}"#)), vec![tool], &host).await;
    assert!(executed.load(Ordering::SeqCst));
    let kinds = host.kinds();
    let request = kinds
        .iter()
        .position(|k| k == "tool_approval_request")
        .unwrap();
    let response = kinds
        .iter()
        .position(|k| k == "tool_approval_response")
        .unwrap();
    let output = kinds
        .iter()
        .position(|k| k == "tool_output_available")
        .unwrap();
    assert!(request < response && response < output);
    assert_eq!(host.approvals.lock().unwrap()[0].input, json!({ "p": 1 }));

    let (executed, tool) = executed_flag();
    let host = TestHost::with_approval(|_| ApprovalDecision::deny("不批"));
    let output = run_with(config(one_call("guarded", "{}")), vec![tool], &host).await;
    assert!(!executed.load(Ordering::SeqCst));
    assert_eq!(tool_output(&output), ToolResultOutput::denied(Some("不批")));

    let host = TestHost::default();
    let hooks = Arc::new(Policy(ToolCallDecision::require_approval("高风险")));
    run_with(
        config(one_call("echo", "{}")).with_hooks(hooks),
        vec![echo_tool("echo")],
        &host,
    )
    .await;
    assert!(host.events().iter().any(
        |e| matches!(e, AgentEvent::ToolApprovalRequest { reason: Some(r), .. } if r == "高风险")
    ));
    // 钩子给出的原因同样会随审批请求交给宿主，供审批策略决策
    assert_eq!(
        host.approvals.lock().unwrap()[0].reason.as_deref(),
        Some("高风险")
    );
}

#[tokio::test]
async fn provider_approval_requests_are_answered_in_the_next_request() {
    let model = ScriptedModel::new(vec![
        Script::Parts(vec![
            StreamPart::ToolCall {
                tool_call_id: "p1".into(),
                tool_name: "web_search".into(),
                input: r#"{"q":"rust"}"#.into(),
                provider_executed: true,
                dynamic: false,
                provider_metadata: None,
            },
            StreamPart::ToolApprovalRequest {
                approval_id: "a1".into(),
                tool_call_id: "p1".into(),
                provider_metadata: None,
            },
            finish(UnifiedFinishReason::Stop),
        ]),
        Script::Parts(text("搜完了")),
    ]);
    let host = TestHost::default();
    let output = run_with(config(model.clone()), vec![], &host).await;

    assert_eq!(model.call_count(), 2);
    let events = host.events();
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolInputAvailable {
            provider_executed: true,
            ..
        }
    )));
    assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolApprovalRequest { tool_name, .. } if tool_name == "web_search")));
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolApprovalResponse {
            provider_executed: true,
            approved: true,
            ..
        }
    )));
    let Message::Tool { content, .. } = &output.messages[2] else {
        panic!("应为工具消息")
    };
    assert!(
        matches!(&content[0], ToolPart::ToolApprovalResponse { approval_id, approved: true, .. } if approval_id == "a1")
    );
}
