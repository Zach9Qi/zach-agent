//! 模型流内报错：按错误类别决定是否重试；失败时不提交失败响应、不执行其中的工具调用

use crate::support::{echo_tool, fn_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zach_agent::{run_agent_loop, AgentContext, LoopConfig, RunError, ToolOutcome};
use zach_ai_core::{
    ErrorInfo, ErrorKind, Message, ModelError, StreamPart, UnifiedFinishReason, Usage,
};

fn stream_error(kind: ErrorKind, message: &str) -> StreamPart {
    StreamPart::Error(ErrorInfo::new(kind, message))
}

#[tokio::test]
async fn stream_errors_prevent_tool_execution_and_response_commit() {
    for reason in [UnifiedFinishReason::ToolCalls, UnifiedFinishReason::Length] {
        // 分别覆盖工具调用前、调用后及收尾后的错误。
        for error_index in 0..=2 {
            let executions = Arc::new(AtomicUsize::new(0));
            let count = executions.clone();
            let tool = fn_tool("effect", move |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok(ToolOutcome::text("已执行")) }
            })
            .shared();
            let mut parts = tool_calls(&[("c1", "effect", "{}")], reason);
            parts.insert(
                error_index,
                stream_error(ErrorKind::InvalidResponse, "模型流报错"),
            );
            let model = ScriptedModel::new(vec![Script::Parts(parts)]);
            let host = TestHost::default();
            let prompt = Message::user("执行工具");
            let error = run_agent_loop(
                vec![prompt.clone()],
                AgentContext {
                    tools: vec![tool],
                    ..Default::default()
                },
                LoopConfig::new(model.clone()),
                &host,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();

            assert!(matches!(
                &error,
                RunError::Model { error, .. }
                    if error.kind() == ErrorKind::InvalidResponse && error.message() == "模型流报错"
            ));
            assert_eq!(executions.load(Ordering::SeqCst), 0);
            assert_eq!(model.call_count(), 1, "协议错误不重试");
            assert_eq!(*host.messages.lock().unwrap(), vec![prompt]);
            assert!(host.approvals.lock().unwrap().is_empty());
            let kinds = host.kinds();
            assert_eq!(kinds.last().map(String::as_str), Some("run_error"));
            assert!(!kinds.iter().any(|kind| kind == "run_finish"));
        }
    }
}

#[tokio::test]
async fn stream_errors_prevent_text_response_commit() {
    let mut parts = text("不能作为成功响应写入历史");
    parts.insert(
        2,
        stream_error(ErrorKind::InvalidResponse, "文本分块解析失败"),
    );
    let model = ScriptedModel::new(vec![Script::Parts(parts)]);
    let host = TestHost::default();
    let prompt = Message::user("你好");
    let result = run_agent_loop(
        vec![prompt.clone()],
        AgentContext::default(),
        LoopConfig::new(model),
        &host,
        CancellationToken::new(),
    )
    .await;

    assert!(matches!(
        &result,
        Err(RunError::Model { error, .. }) if error.message() == "文本分块解析失败"
    ));
    assert_eq!(*host.messages.lock().unwrap(), vec![prompt]);
    assert_eq!(host.kinds().last().map(String::as_str), Some("run_error"));
}

/// 防止回归到"流内错误一律转成不可重试的 Other"：过载以流内事件到达时，
/// 必须与 HTTP 529 一样按策略重试，而不是直接让运行失败
#[tokio::test(start_paused = true)]
async fn retryable_stream_error_events_are_retried_like_their_http_counterparts() {
    let mut overloaded = text("半截");
    overloaded.insert(2, stream_error(ErrorKind::Unavailable, "Overloaded"));
    let model = ScriptedModel::new(vec![Script::Parts(overloaded), Script::Parts(text("好"))]);
    let host = TestHost::default();
    let output = run_agent_loop(
        vec![Message::user("你好")],
        AgentContext::default(),
        LoopConfig::new(model.clone()),
        &host,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(model.call_count(), 2);
    assert_eq!(
        host.kinds().iter().filter(|k| *k == "step_retry").count(),
        1
    );
    assert_eq!(output.messages.last(), Some(&Message::assistant("好")));
}

/// 失败前已完成的轮次（消息与用量）随错误一并返回，计费统计不会因失败而丢失
#[tokio::test]
async fn failure_after_a_completed_step_returns_the_completed_part() {
    let model = ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", "echo", r#"{"text":"x"}"#)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Fail(ModelError::new(ErrorKind::Authentication, "过期")),
    ]);
    let host = TestHost::default();
    let error = run_agent_loop(
        vec![Message::user("你好")],
        AgentContext {
            tools: vec![echo_tool("echo")],
            ..Default::default()
        },
        LoopConfig::new(model),
        &host,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Authentication);
    // 对内层错误透明：直接打印就能看到失败原因
    assert_eq!(error.to_string(), "鉴权失败: 过期");
    let RunError::Model { partial, .. } = error else {
        panic!("应为模型错误");
    };
    assert_eq!(partial.steps, 1);
    assert_eq!(partial.messages.len(), 3, "提示、助手工具调用与工具结果");
    assert_eq!(partial.usage, Usage::simple(10, 5));
    assert!(!partial.aborted);
}
