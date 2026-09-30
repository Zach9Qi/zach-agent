//! 并行/串行执行、批量终止、进度上报与中止时的结果配对

use super::{config, one_call, run_with, tool_output};
use crate::support::{fn_tool, tool_calls, Script, ScriptedModel, TestHost};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Barrier;
use zach_agent::{AgentEvent, SharedTool, ToolExecutionMode, ToolOutcome};
use zach_ai_core::{Message, ToolPart, ToolResultOutput, UnifiedFinishReason};

/// 睡眠指定毫秒后返回自身名字的工具
fn timed(name: &'static str, delay_ms: u64) -> SharedTool {
    fn_tool(name, move |_, _| async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        Ok(ToolOutcome::text(name))
    })
    .shared()
}

/// 最终（非预备）工具输出事件的到达顺序
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
async fn batch_terminates_when_every_tool_asks_to() {
    let stop = fn_tool("stop", |_, _| async {
        Ok(ToolOutcome::text("好").with_terminate(true))
    })
    .shared();
    let model = one_call("stop", "{}");
    let host = TestHost::default();
    run_with(config(model.clone()), vec![stop], &host).await;
    assert_eq!(model.call_count(), 1);
    assert_eq!(host.kinds().last().map(String::as_str), Some("run_finish"));
}

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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
        ["step_finish", "run_abort"]
    );
}
