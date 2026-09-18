# zach-agent

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](Cargo.toml)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](Cargo.toml)

`zach-agent` 是一套基于 Rust 构建的现代化大语言模型（LLM）中间层抽象与智能化 Agent 运行时框架。

本项目致力于提供厂商无关的模型契约抽象、细粒度流式事件协议以及高度可扩展的工具调度运行时，帮助开发者构建高性能、类型安全且易于维护的 AI 智能体应用。

---

## 目录

- [核心特性](#核心特性)
- [架构设计](#架构设计)
- [Crates 模块概览](#crates-模块概览)
- [快速开始](#快速开始)
  - [添加依赖](#添加依赖)
  - [工具注册与调度示例](#工具注册与调度示例)
  - [模型消息与提示词构建](#模型消息与提示词构建)
- [开发与测试](#开发与测试)
- [代码与提交规范](#代码与提交规范)
- [开源协议](#开源协议)

---

## 核心特性

- 🌐 **厂商中立的统一中间形态**：统一抽象不同模型厂商（如 OpenAI、Anthropic 等）的消息、提示词、工具定义与响应结构，业务层一次编写即可在不同底层提供方之间平滑切换。
- ⚡ **细粒度流式生命周期**：不仅支持常规文本输出流，还原生支持推理/思考过程（Reasoning）、工具调用的增量块与生命周期事件，并提供开箱即用的流聚合器（`StreamAccumulator`）。
- 🛠️ **先进的工具调度与懒加载**：
  - **多形态工具支持**：支持原生同步/异步函数工具（`FunctionTool`）与各厂商专用内置工具（`ProviderTool`）。
  - **动态发现与懒加载（Deferred Tools）**：支持仅暴露轻量级元信息，当模型按需调用时再动态激活与拉取完整工具，显著减少提示词上下文（Token）消耗。
  - **内置发现元工具**：自动生成 `__discover_tools` 元工具，模型可主动探测环境中可用的延迟加载工具能力。
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
zach-agent   (编排层: Agent 运行时/ToolRegistry/工具调度与执行)
```

---

## Crates 模块概览

| Crate | 路径 | 说明 |
| :--- | :--- | :--- |
| **`zach-ai-core`** | `crates/zach-ai-core` | **底层抽象契约**。定义 `LanguageModel` 统一接口、`Prompt`/`Message` 结构、流式事件 (`StreamPart`)、流聚合器 (`StreamAccumulator`)、工具协议与调用参数 (`CallOptions`)。 |
| **`zach-ai`** | `crates/zach-ai` | **厂商实现层**。负责将各大模型厂商特有的 API 协议（OpenAI、Anthropic 等）双向转换为 `zach-ai-core` 的统一中间形态。 |
| **`zach-agent`** | `crates/zach-agent` | **Agent 编排与运行时**。提供 `ToolRegistry` 工具注册表、`ToolExecutor` 执行器抽象、动态工具延迟加载机制及调度生命周期管理。 |

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

### 工具注册与调度示例

以下示例演示如何在 `zach-agent` 中定义同步工具、异步工具与延迟加载工具：

```rust
use serde_json::json;
use zach_agent::{ToolExecutor, ToolRegistry};
use zach_ai_core::{FunctionTool, ToolDefinition};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = ToolRegistry::new();

    // 1. 注册同步函数工具
    let calc_def = ToolDefinition::Function(
        FunctionTool::builder("calculate")
            .description("执行简单的数学运算")
            .parameters(json!({
                "type": "object",
                "properties": {
                    "expression": { "type": "string", "description": "数学表达式" }
                },
                "required": ["expression"]
            }))
            .build()?,
    );

    registry.register_sync_fn(calc_def, |_id, args| {
        let expr = args["expression"].as_str().unwrap_or("0");
        Ok(json!({ "result": format!("计算结果: {}", expr) }))
    });

    // 2. 导出提供给大模型的活跃工具定义列表（包含元发现工具）
    let active_tools = registry.export_active_definitions();
    println!("已激活工具数量: {}", active_tools.len());

    // 3. 执行工具调用
    let call_result = registry
        .execute("calculate", "call-001", &json!({ "expression": "1 + 1" }))
        .await?;

    println!("工具返回结果: {:?}", call_result.output);
    Ok(())
}
```

### 模型消息与提示词构建

使用 `zach-ai-core` 构建多模态且厂商中立的 Prompt：

```rust
use zach_ai_core::{Message, Prompt, UserPart};

fn create_sample_prompt() -> Prompt {
    Prompt::new(vec![
        Message::system("你是一个专业的代码审查助手。"),
        Message::user_parts(vec![
            UserPart::text("请帮我审查以下代码片段的并发安全性："),
            UserPart::text("pub async fn execute() { ... }"),
        ]),
    ])
}
```

---

## 开发与测试

运行完整的单元测试与集成测试：

```bash
# 运行工作区所有测试
cargo test

# 仅测试 agent 模块
cargo test -p zach-agent

# 仅测试 ai-core 模块
cargo test -p zach-ai-core
```

---

## 代码与提交规范

本项目遵循严谨的工程与代码规范：

1. **统一中文**：代码注释、文档注释、项目文档以及日常沟通交流统一使用中文。
2. **Git 提交规范**：遵循 Conventional Commits 规范，格式为 `<type>(<可选 scope>): <中文描述>`，例如：
   - `feat(zach-agent): 新增工具延迟加载支持`
   - `fix(zach-ai-core): 修复流式累加器完成原因解析逻辑`
   - `docs: 添加项目中文 README.md 说明文档`
3. **原子化提交（Atomic Commits）**：每次提交保持单一职责，确保独立可编译与测试通过。

---

## 开源协议

本项目采用 [MIT OR Apache-2.0](Cargo.toml) 双重开源许可证授权。
