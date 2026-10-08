//! 三种联调模式、工具回传、失败收尾及虚拟时钟超时。

use super::super::{output::Reporter, runner::run};
use super::{output::BrokenWriter, support::*};
use serde_json::json;
use std::sync::Arc;
use zach_ai_core::{
    Message, StreamPart, ToolChoice, ToolPart, ToolResultOutput, UnifiedFinishReason,
};

#[tokio::test]
async fn one_scenario_uses_the_cli_selected_execution_mode_and_reports_it() {
    let scenario = scenario("stream");
    for mode in ["generate", "stream", "agent"] {
        let config = config(&["--mode", mode]);
        let model = Model::new(vec![Script::Reply(text_reply("回复"))]);
        let (output, buffer) = output();
        run(&config, &scenario, model.clone(), output)
            .await
            .unwrap();
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].prompt.messages, vec![Message::user("请求")]);
        assert_eq!(requests[0].max_output_tokens, Some(4096));
        let logs = buffer.lines();
        assert_eq!(logs[0]["data"]["mode"], mode);
        assert_eq!(
            *model.methods.lock().unwrap(),
            [if mode == "generate" {
                "generate"
            } else {
                "stream"
            }]
        );
        assert_eq!(
            logs.iter().any(|v| v["type"] == "agent_result"),
            mode == "agent"
        );
        assert_eq!(logs.last().unwrap()["type"], "probe_finish");
        assert_eq!(logs.last().unwrap()["data"]["success"], true);
        assert_eq!(
            logs.iter().filter(|v| v["type"] == "model_request").count(),
            1
        );
        assert!(logs
            .iter()
            .any(|v| v["type"] == "model_result"
                && v["data"]["usage"]["input_tokens"]["total"] == 10));
    }
}

#[tokio::test]
async fn agent_executes_add_and_returns_the_result_in_the_next_request() {
    let config = config(&["--mode", "agent"]);
    let scenario = scenario("agent");
    let model = Model::new(vec![
        Script::Reply(tool_reply("call_1")),
        Script::Reply(text_reply("42")),
    ]);
    let (output, buffer) = output();
    run(&config, &scenario, model.clone(), output)
        .await
        .unwrap();
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].tool_choice, Some(ToolChoice::specific("add")));
    assert_eq!(requests[1].tool_choice, Some(ToolChoice::None));
    assert!(requests[1].prompt.messages.iter().any(|message| {
        matches!(message, Message::Tool { content, .. } if content.iter().any(|part| matches!(
            part, ToolPart::ToolResult { tool_call_id, output: ToolResultOutput::Json { value, .. }, .. }
                if tool_call_id == "call_1" && value == &json!({"sum":42})
        )))
    }));
    let result = buffer
        .lines()
        .into_iter()
        .find(|v| v["type"] == "agent_result")
        .unwrap();
    assert_eq!(result["data"]["steps"], 2);
    assert_eq!(result["data"]["usage"]["input_tokens"]["total"], 20);
}

#[tokio::test]
async fn agent_without_a_successful_tool_roundtrip_fails_the_probe() {
    let config = config(&["--mode", "agent"]);
    let scenario = scenario("agent");
    let model = Model::new(vec![Script::Reply(text_reply("42"))]);
    let (output, buffer) = output();
    assert!(run(&config, &scenario, model, output)
        .await
        .unwrap_err()
        .contains("断言失败"));
    assert_eq!(buffer.lines().last().unwrap()["data"]["success"], false);
}

#[tokio::test]
async fn agent_stops_requesting_after_the_configured_call_limit() {
    let config = config(&["--mode", "agent"]);
    let mut scenario = scenario("agent");
    scenario.agent.as_mut().unwrap().max_steps = 2;
    let model = Model::new(vec![
        Script::Reply(tool_reply("call_1")),
        Script::Reply(tool_reply("call_2")),
    ]);
    let (output, _) = output();
    let error = run(&config, &scenario, model.clone(), output)
        .await
        .unwrap_err();
    assert!(error.contains("调用上限"));
    assert_eq!(model.requests.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn every_mode_obeys_the_whole_probe_deadline() {
    for mode in ["generate", "stream", "agent"] {
        let config = config(&["--mode", mode]);
        let mut scenario = scenario(mode);
        scenario.timeout_secs = 1;
        let (output, buffer) = output();
        let start = tokio::time::Instant::now();
        let error = run(
            &config,
            &scenario,
            Model::new(vec![Script::Pending]),
            output,
        )
        .await
        .unwrap_err();
        assert!(error.contains("超过 1 秒"));
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(1));
        assert_eq!(buffer.lines().last().unwrap()["data"]["success"], false);
    }
}

#[tokio::test]
async fn semantic_errors_truncation_and_empty_answers_never_report_success() {
    let config = config(&[]);
    let scenario = scenario("stream");
    let mut truncated = text_reply("半截");
    if let StreamPart::Finish { finish_reason, .. } = &mut truncated[1] {
        finish_reason.unified = UnifiedFinishReason::Length;
    }
    let mut failed = vec![StreamPart::Error {
        message: "错误".into(),
        raw: None,
    }];
    failed.extend(text_reply("诊断"));
    for parts in [truncated, failed, text_reply(""), vec![]] {
        let (output, buffer) = output();
        assert!(run(
            &config,
            &scenario,
            Model::new(vec![Script::Reply(parts)]),
            output
        )
        .await
        .is_err());
        assert_eq!(buffer.lines().last().unwrap()["data"]["success"], false);
    }
}

#[tokio::test]
async fn transport_failure_keeps_partial_output_and_is_not_retried() {
    let config = config(&[]);
    let scenario = scenario("stream");
    let model = Model::new(vec![Script::TransportFailure]);
    let (output, buffer) = output();
    assert!(run(&config, &scenario, model.clone(), output)
        .await
        .is_err());
    assert_eq!(model.requests.lock().unwrap().len(), 1);
    assert!(buffer.lines().iter().any(|v| v["type"] == "partial_result"));
}

#[tokio::test]
async fn closed_output_prevents_a_network_call_from_starting() {
    let config = config(&[]);
    let scenario = scenario("stream");
    let model = Model::new(vec![]);
    let output = Arc::new(Reporter::new(BrokenWriter, String::new()));
    assert!(run(&config, &scenario, model.clone(), output)
        .await
        .is_err());
    assert!(model.requests.lock().unwrap().is_empty());
}
