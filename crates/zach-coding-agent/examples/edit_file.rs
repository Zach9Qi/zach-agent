//! 示例：把编码工具集交给 Agent，跑通"读文件 → 修改 → 汇报"的完整循环
//!
//! 运行：`cargo run -p zach-coding-agent --example edit_file`
//!
//! 本示例不访问网络，也不依赖本机 shell。[`ScriptedModel`] 按对话中已有的工具结果数量
//! 依次发起 `read`、`edit` 调用并最终作答；工具在临时目录组成的工作区里真实读写文件。
//! 系统提示词由 `SystemPrompt` 组装，并带上工作区中 AGENTS.md 的项目约定。
//! 宿主从事件的 `tool_metadata` 中取出 edit 工具给出的统一差异用于展示，模型看不到它。
//!
//! 接入真实模型时只需替换模型实现。

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use zach_agent::{Agent, AgentEvent};
use zach_ai_core::{
    CallOptions, FinishReason, GenerateResult, LanguageModel, LanguageModelStream, Message,
    ModelError, StreamPart, Usage,
};
use zach_coding_agent::{CodingTools, ContextFiles, LocalFileSystem, SystemPrompt, Workspace};

/// 按轮次固定行事的模型：第 1 轮读文件，第 2 轮修改，第 3 轮作答
struct ScriptedModel;

#[async_trait]
impl LanguageModel for ScriptedModel {
    fn provider(&self) -> &str {
        "example"
    }

    fn model_id(&self) -> &str {
        "scripted"
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        Err(ModelError::unsupported("generate", None))
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        let finished_tools = options
            .prompt
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .count();
        let parts = match finished_tools {
            0 => tool_call("call_1", "read", json!({ "path": "greet.py" })),
            1 => tool_call(
                "call_2",
                "edit",
                json!({
                    "path": "greet.py",
                    "edits": [{ "old_text": "print(\"hello\")", "new_text": "print(\"hello, world\")" }],
                }),
            ),
            _ => text_reply("已把问候语改为 hello, world。"),
        };
        Ok(Box::pin(futures::stream::iter(parts.into_iter().map(Ok))))
    }
}

fn tool_call(id: &str, name: &str, input: serde_json::Value) -> Vec<StreamPart> {
    vec![
        StreamPart::ToolCall {
            tool_call_id: id.into(),
            tool_name: name.into(),
            input: input.to_string(),
            provider_executed: false,
            dynamic: false,
            provider_metadata: None,
        },
        finish(FinishReason::tool_calls()),
    ]
}

fn text_reply(text: &str) -> Vec<StreamPart> {
    let id = "text_1".to_string();
    vec![
        StreamPart::TextStart {
            id: id.clone(),
            provider_metadata: None,
        },
        StreamPart::TextDelta {
            id: id.clone(),
            delta: text.into(),
            provider_metadata: None,
        },
        StreamPart::TextEnd {
            id,
            provider_metadata: None,
        },
        finish(FinishReason::stop()),
    ]
}

fn finish(finish_reason: FinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::simple(20, 10),
        finish_reason,
        provider_metadata: None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    std::fs::write(
        dir.path().join("greet.py"),
        "def greet():\n    print(\"hello\")\n",
    )?;
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "Python 代码使用四个空格缩进。\n",
    )?;

    // 本地磁盘 + 本机 shell；本示例的脚本只用到 read 与 edit
    let workspace = Workspace::new(dir.path());
    let tools = CodingTools::local(workspace.clone()).coding();
    // 从工作区根目录向上查找 AGENTS.md / CLAUDE.md，连同工具规则一起组装系统提示词
    let context = ContextFiles::new()
        .discover(&LocalFileSystem, workspace.root())
        .await;
    let system_prompt = SystemPrompt::new(&workspace)
        .with_tools(&tools)
        .with_context_files(context)
        .render();
    println!("[系统提示词]\n{system_prompt}\n");

    let agent = Agent::builder(Arc::new(ScriptedModel))
        .system_prompt(system_prompt)
        .tools(tools)
        .build();

    let mut run = agent.prompt_text("把 greet.py 的问候语改成 hello, world")?;
    while let Some(event) = run.next().await {
        match event {
            AgentEvent::ToolInputAvailable {
                tool_name, input, ..
            } => println!("[调用工具] {tool_name} {input}"),
            AgentEvent::ToolOutputAvailable {
                output,
                preliminary: false,
                tool_metadata,
                ..
            } => {
                println!("[工具结果] {output:?}");
                let diff = tool_metadata
                    .as_ref()
                    .and_then(|details| details["diff"].as_str());
                if let Some(diff) = diff {
                    println!("[宿主展示的差异]\n{diff}");
                }
            }
            AgentEvent::ToolOutputError { error_text, .. } => println!("[工具失败] {error_text}"),
            AgentEvent::TextDelta { delta, .. } => println!("[模型回复] {delta}"),
            _ => {}
        }
    }

    println!(
        "\n修改后的文件：\n{}",
        std::fs::read_to_string(dir.path().join("greet.py"))?
    );
    Ok(())
}
