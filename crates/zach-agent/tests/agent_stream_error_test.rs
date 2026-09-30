//! 模型流内报错时，不提交失败响应，也不执行其中的工具调用

#[path = "support/mock.rs"]
mod mock;

use mock::{fn_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zach_agent::{run_agent_loop, AgentContext, AgentError, LoopConfig, ToolOutcome};
use zach_ai_core::{Message, ModelError, StreamPart, UnifiedFinishReason};

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
                StreamPart::Error {
                    message: "模型流报错".into(),
                    raw: None,
                },
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

            assert!(
                matches!(error, AgentError::Model(ModelError::Other(message)) if message == "模型流报错")
            );
            assert_eq!(executions.load(Ordering::SeqCst), 0);
            assert_eq!(model.call_count(), 1);
            assert_eq!(*host.messages.lock().unwrap(), vec![prompt]);
            assert!(host.approvals.lock().unwrap().is_empty());
            let kinds = host.kinds();
            assert_eq!(kinds.last().map(String::as_str), Some("run-error"));
            assert!(!kinds.iter().any(|kind| kind == "run-finish"));
        }
    }
}

#[tokio::test]
async fn stream_errors_prevent_text_response_commit() {
    let mut parts = text("不能作为成功响应写入历史");
    parts.insert(
        2,
        StreamPart::Error {
            message: "文本分块解析失败".into(),
            raw: None,
        },
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

    assert!(
        matches!(result, Err(AgentError::Model(ModelError::Other(message))) if message == "文本分块解析失败")
    );
    assert_eq!(*host.messages.lock().unwrap(), vec![prompt]);
    assert_eq!(host.kinds().last().map(String::as_str), Some("run-error"));
}
