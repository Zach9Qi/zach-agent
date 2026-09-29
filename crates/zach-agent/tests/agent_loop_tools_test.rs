//! 低层循环：工具校验、拒绝、审批、截断、终止、并发、进度与中止

#[path = "support/mock.rs"]
mod mock;

use async_trait::async_trait;
use mock::{echo_tool, finish, fn_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;
use tokio_util::sync::CancellationToken;
use zach_agent::{
    run_agent_loop, AgentContext, AgentEvent, AgentHooks, ApprovalDecision, LoopConfig,
    RetryPolicy, RunOutput, SharedTool, ToolCallDecision, ToolCallInfo, ToolError,
    ToolExecutionMode, ToolOutcome,
};
use zach_ai_core::{
    AssistantPart, Message, StreamPart, ToolPart, ToolResultOutput, UnifiedFinishReason,
};

async fn run_with(config: LoopConfig, tools: Vec<SharedTool>, host: &TestHost) -> RunOutput {
    let context = AgentContext {
        tools,
        ..Default::default()
    };
    let prompts = vec![Message::user("开始")];
    run_agent_loop(prompts, context, config, host, CancellationToken::new())
        .await
        .unwrap()
}

fn one_call(name: &str, input: &str) -> Arc<ScriptedModel> {
    ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", name, input)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("收到")),
    ])
}

fn config(model: Arc<ScriptedModel>) -> LoopConfig {
    LoopConfig::new(model).with_retry(RetryPolicy::none())
}

fn tool_output(output: &RunOutput) -> ToolResultOutput {
    output
        .messages
        .iter()
        .find_map(|message| match message {
            Message::Tool { content, .. } => content.iter().find_map(|part| match part {
                ToolPart::ToolResult { output, .. } => Some(output.clone()),
                _ => None,
            }),
            _ => None,
        })
        .expect("缺少工具结果")
}

fn executed_flag() -> (Arc<AtomicBool>, SharedTool) {
    let flag = Arc::new(AtomicBool::new(false));
    let seen = flag.clone();
    let tool = fn_tool("guarded", move |_, _| {
        seen.store(true, Ordering::SeqCst);
        async { Ok(ToolOutcome::text("已执行")) }
    });
    (flag, tool.needs_approval().shared())
}

#[tokio::test]
async fn unknown_tools_and_malformed_input_report_input_errors() {
    for (name, input, expected) in [
        ("missing", "{}", "不存在"),
        ("echo", "{bad", "JSON 对象"),
        ("echo", "[1,2]", "JSON 对象"),
    ] {
        let model = one_call(name, input);
        let host = TestHost::default();
        let output = run_with(config(model.clone()), vec![echo_tool("echo")], &host).await;

        assert!(host.kinds().contains(&"tool-input-error".to_string()));
        assert!(!host.kinds().contains(&"tool-output-error".to_string()));
        assert!(
            matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains(expected))
        );
        assert_eq!(model.call_count(), 2);
    }
}

#[tokio::test]
async fn failing_tools_emit_output_errors() {
    let tool = fn_tool("boom", |_, _| async { Err(ToolError::failed("炸了")) }).shared();
    let host = TestHost::default();
    let output = run_with(config(one_call("boom", "{}")), vec![tool], &host).await;
    assert!(host.events().iter().any(
        |e| matches!(e, AgentEvent::ToolOutputError { error_text, .. } if error_text == "炸了")
    ));
    assert_eq!(tool_output(&output), ToolResultOutput::error_text("炸了"));
}

#[tokio::test]
async fn panicking_tools_become_error_results_without_killing_siblings() {
    let panicking = fn_tool("panic", |_, _| async { panic!("工具内部崩溃") }).shared();
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "panic", "{}"), ("c2", "echo", r#"{"a":1}"#)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("收到")),
    ]);
    let host = TestHost::default();
    let output = run_with(
        config(model.clone()),
        vec![panicking, echo_tool("echo")],
        &host,
    )
    .await;

    let results: Vec<(String, ToolResultOutput)> = output
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ToolPart::ToolResult {
                tool_call_id,
                output,
                ..
            } => Some((tool_call_id.clone(), output.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2, "每个工具调用都应有对应结果");
    assert!(
        matches!(&results[0].1, ToolResultOutput::ErrorText { value, .. } if value.contains("panic") && value.contains("工具内部崩溃")),
        "panic 应转为错误结果: {:?}",
        results[0].1
    );
    assert_eq!(results[1].1, ToolResultOutput::json(json!({"a": 1})));
    assert!(host.kinds().contains(&"tool-output-error".to_string()));
    assert_eq!(host.kinds().last().map(String::as_str), Some("run-finish"));
    assert_eq!(model.call_count(), 2);
}

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
        .position(|k| k == "tool-approval-request")
        .unwrap();
    let response = kinds
        .iter()
        .position(|k| k == "tool-approval-response")
        .unwrap();
    let output = kinds
        .iter()
        .position(|k| k == "tool-output-available")
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
async fn truncated_responses_do_not_execute_tools() {
    let (executed, tool) = executed_flag();
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "guarded", "{\"p\":")],
            UnifiedFinishReason::Length,
        )),
        Script::Parts(text("重来")),
    ]);
    let host = TestHost::default();
    let output = run_with(config(model), vec![tool], &host).await;
    assert!(!executed.load(Ordering::SeqCst));
    assert!(
        matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains("长度上限"))
    );
}

/// 历史中的工具调用入参
fn replayed_tool_inputs(output: &RunOutput) -> Vec<serde_json::Value> {
    output
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|part| match part {
            AssistantPart::ToolCall { input, .. } => Some(input.clone()),
            _ => None,
        })
        .collect()
}

/// 非法、截断或非对象的入参不得原样进入对话记录，否则下一轮请求会被厂商拒绝
#[tokio::test]
async fn rejected_tool_inputs_are_replayed_as_empty_objects() {
    let cases = [
        ("{bad", UnifiedFinishReason::ToolCalls, "JSON 对象"),
        ("[1,2]", UnifiedFinishReason::ToolCalls, "JSON 对象"),
        (r#"{"a":"#, UnifiedFinishReason::Length, "长度上限"),
    ];
    for (input, reason, expected) in cases {
        let model = ScriptedModel::new(vec![
            Script::Parts(tool_calls(&[("c1", "echo", input)], reason)),
            Script::Parts(text("收到")),
        ]);
        let host = TestHost::default();
        let output = run_with(config(model), vec![echo_tool("echo")], &host).await;

        assert_eq!(
            replayed_tool_inputs(&output),
            vec![json!({})],
            "输入 {input:?} 回放后应归一为空对象"
        );
        assert!(
            matches!(tool_output(&output), ToolResultOutput::ErrorText { value, .. } if value.contains(expected)),
            "输入 {input:?} 应配对含 {expected:?} 的错误结果"
        );
    }
}

/// 截断只影响执行策略，不改变回放：完整的入参照常保留
#[tokio::test]
async fn truncated_but_complete_inputs_are_replayed_intact() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "echo", r#"{"a":1}"#)],
            UnifiedFinishReason::Length,
        )),
        Script::Parts(text("收到")),
    ]);
    let host = TestHost::default();
    let output = run_with(config(model), vec![echo_tool("echo")], &host).await;
    assert_eq!(replayed_tool_inputs(&output), vec![json!({"a": 1})]);
}

#[tokio::test]
async fn batch_terminates_when_every_tool_asks_to() {
    let stop = fn_tool("stop", |_, _| async {
        Ok(ToolOutcome::text("好").with_terminate(true))
    })
    .shared();
    let model = one_call("stop", "{}");
    let host = TestHost::default();
    run_with(config(model.clone()), vec![stop], &host).await;
    assert_eq!(model.call_count(), 1);
    assert_eq!(host.kinds().last().map(String::as_str), Some("run-finish"));
}

fn timed(name: &'static str, delay_ms: u64) -> SharedTool {
    fn_tool(name, move |_, _| async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        Ok(ToolOutcome::text(name))
    })
    .shared()
}

fn output_order(host: &TestHost) -> Vec<String> {
    host.events()
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::ToolOutputAvailable {
                tool_call_id,
                preliminary: false,
                ..
            } => Some(tool_call_id),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn parallel_tools_run_concurrently_but_results_keep_source_order() {
    let barrier = Arc::new(Barrier::new(2));
    let rendezvous = |name: &'static str| {
        let barrier = barrier.clone();
        fn_tool(name, move |_, _| {
            let barrier = barrier.clone();
            async move {
                barrier.wait().await;
                Ok(ToolOutcome::text(name))
            }
        })
        .shared()
    };
    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &[("a", "left", "{}"), ("b", "right", "{}")],
        UnifiedFinishReason::ToolCalls,
    ))]);
    let host = TestHost::default();
    let run = run_with(
        config(model),
        vec![rendezvous("left"), rendezvous("right")],
        &host,
    );
    tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("并行工具应同时运行");

    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &[("a", "slow", "{}"), ("b", "fast", "{}")],
        UnifiedFinishReason::ToolCalls,
    ))]);
    let host = TestHost::default();
    let output = run_with(
        config(model),
        vec![timed("slow", 40), timed("fast", 0)],
        &host,
    )
    .await;
    assert_eq!(output_order(&host), ["b", "a"]);
    let Message::Tool { content, .. } = &output.messages[2] else {
        panic!("应为工具消息")
    };
    assert!(
        matches!(&content[0], ToolPart::ToolResult { tool_call_id, .. } if tool_call_id == "a")
    );
}

#[tokio::test]
async fn sequential_mode_runs_tools_one_by_one() {
    let calls = [("a", "slow", "{}"), ("b", "fast", "{}")];
    let model = ScriptedModel::new(vec![Script::Parts(tool_calls(
        &calls,
        UnifiedFinishReason::ToolCalls,
    ))]);
    let host = TestHost::default();
    let config = config(model).with_tool_execution(ToolExecutionMode::Sequential);
    run_with(config, vec![timed("slow", 40), timed("fast", 0)], &host).await;
    assert_eq!(output_order(&host), ["a", "b"]);
}

#[tokio::test]
async fn progress_reports_become_preliminary_outputs() {
    let tool = fn_tool("work", |_, ctx| async move {
        ctx.report_progress(ToolResultOutput::text("50%"));
        Ok(ToolOutcome::text("完成"))
    })
    .shared();
    let host = TestHost::default();
    run_with(config(one_call("work", "{}")), vec![tool], &host).await;
    let outputs: Vec<(bool, ToolResultOutput)> = host
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::ToolOutputAvailable {
                output,
                preliminary,
                ..
            } => Some((preliminary, output)),
            _ => None,
        })
        .collect();
    assert_eq!(
        outputs,
        [
            (true, ToolResultOutput::text("50%")),
            (false, ToolResultOutput::text("完成"))
        ]
    );
}

#[tokio::test]
async fn abort_during_tool_execution_pairs_every_call_with_a_result() {
    let tool = fn_tool("hang", |_, ctx| async move {
        ctx.cancellation_token().cancel();
        futures::future::pending::<()>().await;
        Ok(ToolOutcome::text("不会到达"))
    })
    .shared();
    let host = TestHost::default();
    let output = run_with(config(one_call("hang", "{}")), vec![tool], &host).await;
    assert!(output.aborted);
    assert_eq!(
        tool_output(&output),
        ToolResultOutput::error_text("操作已中止")
    );
    assert_eq!(
        host.kinds()[host.kinds().len() - 2..],
        ["step-finish", "run-abort"]
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
