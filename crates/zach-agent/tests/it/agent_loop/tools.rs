//! 低层循环的工具调度：共享夹具，具体行为按关注点拆分到子模块
//!
//! - `errors`：入参/输出错误与 panic 兜底
//! - `replay`：被拒入参在对话历史中的回放归一
//! - `policy`：钩子放行/拒绝/改写与本地、厂商两类审批
//! - `scheduling`：并行/串行、批量终止、进度与中止配对

mod errors;
mod policy;
mod replay;
mod scheduling;

use crate::support::{fn_tool, text, tool_calls, Script, ScriptedModel, TestHost};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zach_agent::{
    run_agent_loop, AgentContext, LoopConfig, RetryPolicy, RunOutput, SharedTool, ToolOutcome,
};
use zach_ai_core::{Message, ToolPart, ToolResultOutput, UnifiedFinishReason};

/// 以给定配置与工具集跑一次循环，提示固定为一条用户消息
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

/// 第一轮发起一次工具调用，第二轮回复文本
fn one_call(name: &str, input: &str) -> Arc<ScriptedModel> {
    ScriptedModel::new(vec![
        Script::Parts(tool_calls(
            &[("c1", name, input)],
            UnifiedFinishReason::ToolCalls,
        )),
        Script::Parts(text("收到")),
    ])
}

/// 关闭重试的默认配置
fn config(model: Arc<ScriptedModel>) -> LoopConfig {
    LoopConfig::new(model).with_retry(RetryPolicy::none())
}

/// 历史中的第一个工具结果
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

/// 需要审批的工具及其"是否被执行"标记
fn executed_flag() -> (Arc<AtomicBool>, SharedTool) {
    let flag = Arc::new(AtomicBool::new(false));
    let seen = flag.clone();
    let tool = fn_tool("guarded", move |_, _| {
        seen.store(true, Ordering::SeqCst);
        async { Ok(ToolOutcome::text("已执行")) }
    });
    (flag, tool.needs_approval().shared())
}
