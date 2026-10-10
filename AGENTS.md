# 项目规范

## 1. 语言规范
- **统一中文**：代码注释、文档注释、项目文档（README、设计文档等）、对话沟通均统一使用中文。

## 2. Git 提交规范 (Conventional Commits)
- **提交格式**：`<type>(<可选 scope>): <中文描述>`
- **描述规范**：描述部分必须使用规范中文，清晰概括变更意图。
- **常用类型**：`feat`（新功能）、`fix`（修缺陷）、`docs`（文档）、`refactor`（重构）、`perf`（性能）、`test`（测试）、`chore`（构建/杂项）。

## 3. 原子化提交原则 (Atomic Commits)
- **单一职责**：一次提交仅聚焦一个独立、完整的逻辑变更，严禁将多个不同关注点的改动（如“功能开发 + Bug 修复 + 格式化”）混入同一个 commit。
- **精准暂存**：按文件 `git add <file>` 或按代码块 `git add -p` 分批暂存提交，严禁盲目执行 `git add .`。
- **保持自洽**：拆分提交时确保每次 commit 后的代码均可独立编译且测试通过。

## 4. Rust 模块组织规范（严禁 `mod.rs`）
- **Rust 2018+ 风格**：当模块包含子模块时，入口必须是同名 `.rs` 文件（置于父目录），子模块置于同名子目录中。
  - ✅ 正确：`src/parser.rs` 与 `src/parser/ast.rs`
  - ❌ 严禁：`src/parser/mod.rs`
- **模块入口职责**：在入口 `.rs` 文件中声明子模块（`pub mod xxx;`），并按需对外 re-export 常用类型（`pub use xxx::*`）。

## 5. 单文件长度与单一职责
- **行数控制**：单个源码文件建议控制在 **300 行以内**，硬性上限不得超过 **500 行**。
- **职责拆分**：复杂模块应解耦拆分，例如：
  - 错误定义拆分至 `error.rs`
  - 数据模型/结构体拆分至 `types.rs`
  - 辅助工具函数拆分至 `utils.rs`

## 6. 多 Crate 分层架构与依赖约束
遵循 Cargo Workspace 单向分层依赖，严禁反向依赖与循环依赖：
```text
zach-ai-core (底层契约: Trait/基础模型/Error)
  ▲
zach-ai      (实现层: API客户端/Provider/数据流)
  ▲
zach-agent   (编排层: Agent运行时/状态机/工具调度)
```

## 7. 可见性与代码复用
- **最小可见性**：优先使用 `pub(crate)` 或 `pub(super)`，非必要不暴露为 `pub`。
- **公共抽象下沉**：跨 crate 共用的 Trait 和契约沉淀到 `*-core`，面向接口编程。
- **门面模式（Façade）**：在 crate 的 `lib.rs` 中精选导出关键类型，为外部提供简洁统一的导入路径。

## 8. 测试规范

### 8.1 单元测试 vs 集成测试的判定
- **集成测试（`tests/`）**：验证 crate 的**对外契约**，只能通过公开 API 访问；这是默认选择。
- **内嵌单元测试（`#[cfg(test)]`）**：仅用于 `pub(crate)` / `pub(super)` 的内部逻辑与不变量（如内部状态机、槽位维护、队列语义、算术边界），这些行为在公开 API 层只能间接观察。
- **严禁为了可测性扩大可见性**：不得把 `pub(crate)` 升为 `pub` 只为让 `tests/` 能访问；需要测私有逻辑就内嵌测。

### 8.2 集成测试布局：单二进制
每个 crate 的集成测试是**一个**测试目标 `tests/it/main.rs`，按被测主题拆分子模块，不允许在 `tests/` 根下平铺多个 `.rs`：
```text
tests/it/main.rs            ← 仅声明 mod，附一句 //! 说明
tests/it/support.rs         ← 共享测试替身（ScriptedModel、TestHost 等）
tests/it/agent.rs           ← 主题命名，不加 _test 后缀
tests/it/agent_loop.rs      ← 子模块过大时按 2018 风格再拆：
tests/it/agent_loop/tools.rs
tests/it/agent_loop/tools/errors.rs
```
理由：每个 `tests/*.rs` 都是独立 crate，会重复编译夹具、需要 `#[path]` 与 `allow(dead_code)` 兜底、成倍增加链接时间。单二进制下夹具是普通模块，`cargo test -p <crate> agent_loop::` 可按模块过滤。
- **禁止进程级全局状态**：所有用例共享一个进程并行执行，不得在测试里 `set_var`、安装全局 `tracing` subscriber 或依赖静态可变量。
- **测试文件的长度规范**：按被测主题拆分，而不是按行数拆分；单文件建议 **500 行以内**，硬性上限 **800 行**。
  测试的夹具与多行断言天然比源码冗长，第 5 节的 300 / 500 行只约束源码文件（含内嵌单元测试后的总长度，见 8.3）。
  严禁为凑行数拆出单用例模块，或在子模块之间复制夹具——共享夹具下沉到 `support.rs` / `test_support.rs`。

### 8.3 内嵌单元测试的位置
- 默认写在源文件尾部：`#[cfg(test)] mod tests { use super::*; … }`。
- 若会使源文件超过 300 行，拆到同名目录下的 `tests.rs`，源文件中声明 `#[cfg(test)] mod tests;`（如 `agent/host.rs` + `agent/host/tests.rs`）。

### 8.4 命名与文档
- 用例名是**行为句式** snake_case，读起来是一条需求：`sealed_tool_ignores_later_deltas_and_flag_changes`；禁止 `test_xxx`、`case1` 之类。
- 每个测试文件首行 `//!` 一句话说明覆盖范围；非显而易见的用例加 `///` 说明其防范的回归。

### 8.5 确定性：虚拟时钟
- 任何依赖 `sleep` / 超时 / 退避的用例必须用 `#[tokio::test(start_paused = true)]`（tokio `test-util` 作为 `dev-dependency`），禁止用真实等待推断调度顺序。
- 库代码中的延时统一走 `tokio::time`（不得用 `std::thread::sleep` 或 `std::time::Instant` 计时），否则虚拟时钟无法推进。
- `timeout(WAIT, …)` 仅作防悬挂守卫使用，不承担断言语义。
- 测试替身（模型、宿主、工具）必须是纯 async 的（`futures::stream::pending` 等），不得起真实线程或做 IO。

### 8.6 依赖与示例
- 只在测试中使用的 crate 或 feature 一律放 `[dev-dependencies]`，不得混入 `[dependencies]`。
- 面向用户的端到端用法放 `examples/`，示例必须可离线运行（自带最小模型实现），并由 `cargo clippy --all-targets` 门禁覆盖。
- 提交前统一执行 `cargo run -p xtask -- check`（fmt + clippy + test），本地与 CI 同一入口。
