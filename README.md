# zach-agent

[![CI](https://github.com/Zach9Qi/zach-agent/actions/workflows/ci.yml/badge.svg)](https://github.com/Zach9Qi/zach-agent/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](Cargo.toml)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](Cargo.toml)

`zach-agent` 是一套基于 Rust 构建的现代化大语言模型（LLM）中间层抽象与智能化 Agent 运行时框架。

本项目致力于提供厂商无关的模型契约抽象与细粒度流式事件协议，帮助开发者构建高性能、类型安全且易于维护的 AI 智能体应用。

---

## 目录

- [核心特性](#核心特性)
- [架构设计](#架构设计)
- [Crates 模块概览](#crates-模块概览)
- [快速开始](#快速开始)
  - [添加依赖](#添加依赖)
  - [模型消息与提示词构建](#模型消息与提示词构建)
- [开发与测试](#开发与测试)
- [代码与提交规范](#代码与提交规范)
- [开源协议](#开源协议)

---

## 核心特性

- 🌐 **厂商中立的统一中间形态**：统一抽象不同模型厂商（如 OpenAI、Anthropic 等）的消息、提示词、工具定义与响应结构，业务层一次编写即可在不同底层提供方之间平滑切换。
- ⚡ **细粒度流式生命周期**：不仅支持常规文本输出流，还原生支持推理/思考过程（Reasoning）、工具调用的增量块与生命周期事件，并提供开箱即用的流聚合器（`StreamAccumulator`）。
- 🔒 **坚固的类型安全与高性能**：依托 Rust 严格的类型检查与基于 `tokio` 的高并发异步运行时，保障多智能体交互的稳定性与低延迟。

---

## 架构设计

本项目遵循清晰的 Cargo Workspace 单向分层设计原则，严禁反向与循环依赖：

```text
zach-ai-core (底层契约: Trait/基础模型/流式事件/Error)
      ▲
      │ (依赖)
zach-ai      (适配层: 各 LLM 厂商 API 客户端/协议映射)
      ▲
      │ (依赖)
zach-agent   (编排层: Agent 运行时)
```

---

## Crates 模块概览

| Crate | 路径 | 说明 |
| :--- | :--- | :--- |
| **`zach-ai-core`** | `crates/zach-ai-core` | **底层抽象契约**。定义 `LanguageModel` 统一接口、`Prompt`/`Message` 结构、流式事件 (`StreamPart`)、流聚合器 (`StreamAccumulator`)、工具协议与调用参数 (`CallOptions`)。 |
| **`zach-ai`** | `crates/zach-ai` | **厂商实现层**。已实现 OpenAI Responses API、Chat Completions 兼容协议、Anthropic Messages API 和模型档案目录。 |
| **`zach-agent`** | `crates/zach-agent` | **Agent 编排与运行时**。提供低层循环（`run_agent_loop`）与有状态的 `Agent` 句柄：多轮工具调用、并行/串行工具执行、人工审批、插队/追加消息、策略钩子、失败重试与中止，运行过程以 `AgentEvent` 事件流透出。 |

---

## 快速开始

### 添加依赖

在你的 `Cargo.toml` 中按需引入依赖：

```toml
[dependencies]
zach-ai-core = { path = "crates/zach-ai-core" }
zach-agent = { path = "crates/zach-agent" }
# 若需要启用具体厂商适配器：
# zach-ai = { path = "crates/zach-ai" }
```

### 模型消息与提示词构建

真实模型接入使用 `zach_ai::OpenAiResponsesModel::new(api_key, model_id)` 或
`zach_ai::AnthropicMessagesModel::new(api_key, model_id)`，可直接传给
`Agent::builder(Arc::new(model))`。默认 feature 已启用全部适配器和 Rustls；
支持范围、流式调用和请求配置见 [OpenAI Responses 使用说明](docs/openai-responses.md)、
[OpenAI Chat Completions 使用说明](docs/openai-chat.md) 和
[Anthropic Messages 使用说明](docs/anthropic-messages.md)。

使用 `zach-ai-core` 构建多模态且厂商中立的 Prompt：

```rust
use zach_ai_core::{Message, Prompt, UserPart};

fn create_sample_prompt() -> Prompt {
    Prompt::from_messages(vec![
        Message::system("你是一个专业的代码审查助手。"),
        Message::User {
            content: vec![
                UserPart::text("请帮我审查以下代码片段的并发安全性："),
                UserPart::text("pub async fn execute() { ... }"),
            ],
            provider_options: None,
        },
    ])
}
```

### 运行 Agent

`Agent` 持有对话记录与配置，每次 `prompt` 返回一个 `AgentRun`：它本身是 `Stream<Item = AgentEvent>`，消费完事件后可通过 `outcome()` 取得本次新增的消息与累计用量。`Agent` 可廉价克隆，运行中可在其他任务里调用 `steer`、`follow_up`、`abort`、`respond_approval`。

```rust
use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;
use zach_agent::{typed_tool, Agent, AgentEvent, ApprovalDecision, ToolContext, ToolError, ToolOutcome, TypedTool};
use zach_ai_core::LanguageModel;

#[derive(Deserialize, JsonSchema)]
struct WeatherInput {
    /// 城市名
    city: String,
}

struct Weather;

#[async_trait::async_trait]
impl TypedTool for Weather {
    type Input = WeatherInput;

    fn name(&self) -> &str { "get_weather" }
    fn description(&self) -> &str { "查询城市天气" }

    async fn call(&self, input: WeatherInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::text(format!("{} 晴，25℃", input.city)))
    }
}

async fn chat(model: Arc<dyn LanguageModel>) -> Result<(), zach_agent::AgentError> {
    let agent = Agent::builder(model)
        .system_prompt("你是天气助手")
        .tool(typed_tool(Weather))
        .build();

    let mut run = agent.prompt_text("北京今天天气怎么样？")?;
    while let Some(event) = run.next().await {
        match event {
            AgentEvent::TextDelta { delta, .. } => print!("{delta}"),
            AgentEvent::ToolApprovalRequest { approval_id, .. } => {
                agent.respond_approval(&approval_id, ApprovalDecision::approve())?;
            }
            _ => {}
        }
    }
    let output = run.outcome().await?;
    println!("\n本次共 {} 轮", output.steps);
    Ok(())
}
```

运行流程要点：

- **轮次**：一轮 = 一次模型响应 + 执行其中的工具调用。有工具结果、插队消息或钩子要求继续时进入下一轮；本应结束时若有追加消息也会继续。
- **启动与排队消息**：`prompt()` 与 `continue_run()` 使用同一启动流程，校验、取初始消息与占用运行在一次持锁期间完成，返回 Busy 或校验失败不会取走队列消息。启动时按 `OneAtATime` / `All` 取插队消息，启动后新增的插队消息留到下一轮；追加消息仍在本轮结束后注入，仅从助手消息继续且无插队消息时提前取用。
- **工具**：默认先依次准备（校验入参、`before_tool_call`、审批），再并发执行，结果按模型给出的顺序写回；`ToolExecutionMode::Sequential` 或任一工具声明串行时整批串行。模型因长度上限截断时不执行工具。
- **钩子**（`AgentHooks`）：`transform_context`、`prepare_request`、`before_tool_call`、`after_tool_call`、`finish_turn`、`prepare_next_turn`，可逐轮替换模型、调用参数与上下文。
- **审批**（`ApprovalHandler`）：工具声明 `needs_approval` 或钩子返回 `RequireApproval` 时，请求先交给审批处理器选择路径——`ApprovalRoute::Decided` 直接放行/拒绝，`ApprovalRoute::Ask` 发出 `ToolApprovalRequest` 事件并等待 `respond_approval`（可带超时，`None` 表示永不超时）。默认 `InteractiveApproval` 一律转人工；无人值守场景用 `DenyAll` / `ApproveAll`，或以闭包实现白名单：`.approval(|req: &ApprovalRequest| if req.tool_name == "ls" { ApprovalRoute::approve() } else { ApprovalRoute::ask() })`。
  - 未消费事件流（调用 `outcome()` 或丢弃 `AgentRun`）时，转人工的请求会被自动拒绝而不是让运行悬挂；批处理请显式配置处理器。
- **失败与中止**：可重试的模型错误按 `RetryPolicy` 退避重试并发出 `StepRetry`；重试用尽发出 `RunError`，失败那一轮不写入对话记录，可直接 `continue_run()` 重试。中止发出 `RunAbort`，已生成的文本保留，未完成的工具调用补"已中止"结果。
- **事件背压**：`AgentRun` 的普通事件队列有界，正常运行时需持续消费；中止会打断发送等待，即使保留句柄且不消费，也不会因满队列卡住收尾。中止后的普通事件仅尽力投递，队列满时可能丢弃；`RunAbort` / `RunError` / `RunFinish` 独立保存，恢复消费后在已排队的普通事件之后产出。

不需要状态管理时，可直接调用低层 `run_agent_loop`，并自行实现 `LoopHost` 接收事件、提供排队消息与审批答复（转人工时由宿主发出 `ApprovalRequest::to_event()`）。

---

## 开发与测试

真实 API 联调使用统一入口：

```powershell
cargo xtask probe --help
cargo xtask probe anthropic                   # 全套：generate、stream、agent
cargo xtask probe chat agent                  # 单个模式
cargo xtask probe responses stream --scenario path/to/my.json
```

在仓库根目录的 `.env.local` 中按厂商填写模型 ID 和凭据（可复制 `.env.local.example`，本地文件已被 Git 忽略）：
`responses/chat` 读取 `PROBE_OPENAI_*`，`anthropic` 读取 `PROBE_ANTHROPIC_*`，缺失时回退 `PROBE_*`。
协议与模式是位置参数；省略模式时依次运行内置套件，同一场景可重复用于
`generate/stream/agent`，无需为执行方式复制输入文件。
场景中的 `request` 沿用标准 `CallOptions`，可组合文本、多图片、多文件与推理配置；
本地附件直接在消息文件块中填写 `path`，加载器自动补齐标准 `data`。
`expect` 验证内容、事件、工具执行及跨轮回传。配置与断言说明见 [真实 API 联调说明](docs/probe.md)。
输出 Schema 可从 Rust 类型通过 `cargo xtask probe-schemas` 自动生成，多个场景通过
`schema_file` 复用，无需手写或重复粘贴结构定义。
真实调用由开发者手动执行，日常检查和测试不访问厂商 API。

运行完整的单元测试与集成测试：

```bash
# 运行工作区所有测试
cargo test

# 仅测试 agent 模块
cargo test -p zach-agent

# 仅测试 ai-core 模块
cargo test -p zach-ai-core
```

提交前运行统一门禁（格式检查 → Clippy → 测试，任一失败即停止），本地与 CI 共用同一入口：

```bash
cargo xtask check
```

---

## 代码与提交规范

本项目遵循严谨的工程与代码规范：

1. **统一中文**：代码注释、文档注释、项目文档以及日常沟通交流统一使用中文。
2. **Git 提交规范**：遵循 Conventional Commits 规范，格式为 `<type>(<可选 scope>): <中文描述>`，例如：
   - `feat(zach-agent): 新增运行时事件模型`
   - `fix(zach-ai-core): 修复流式累加器完成原因解析逻辑`
   - `docs: 添加项目中文 README.md 说明文档`
3. **原子化提交（Atomic Commits）**：每次提交保持单一职责，确保独立可编译与测试通过。

---

## 开源协议

本项目采用 [MIT OR Apache-2.0](Cargo.toml) 双重开源许可证授权。
