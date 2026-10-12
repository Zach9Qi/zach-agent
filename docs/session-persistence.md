# 会话持久化与上下文压缩设计

状态：已实施，默认的文件存储尚未实现（见文末"实施状态"）。适用范围：`zach-agent`（契约补充）与 `zach-coding-agent`（会话层）。

## 目标与非目标

目标：

- **崩溃安全**：每条消息写入对话记录时即落盘，任何时刻进程被杀，重新打开后对话记录与磁盘上的工作区改动一致。
- **单一事实来源**：会话日志是唯一真相，模型上下文由日志按纯函数投影得出，`Agent` 只是一次运行的执行器。
- **历史不可变**：压缩、回退、分叉都以追加条目表达，从不改写已写入的内容。
- **可替换的存储**：存储放在 trait 之后，默认提供 JSONL 文件实现。
- **可演进的格式**：带版本号，旧会话永远可以加载。

本期非目标：跨会话全文检索、多进程同时写同一会话、云端同步、会话浏览界面、blob 垃圾回收。

## 分层与职责

```text
zach-agent            提交通知：每条消息写入对话记录后，有序、不可取消地通知宿主（附带用量与工具详情）
     ▲
zach-coding-agent
  session/            Session（驱动 Agent）、条目模型、投影、压缩
  session/store       SessionStore trait、MemoryStore（进程内）；文件存储待实现
```

- `zach-agent` 不感知会话、文件与格式，只新增"提交通知"这一项契约。
- 会话层先作为 `zach-coding-agent` 的 `session` 模块实现，内部不依赖 `tools` 模块（压缩中的文件追踪通过配置注入工具名）。
  出现第二个使用方时再提升为独立 crate，届时只需搬迁模块。

## 一、zach-agent 契约补充：提交通知

### 现状

循环在 [`Runner::commit`](../crates/zach-agent/src/agent_loop.rs) 中调用 `LoopHost::on_message(&Message)`，覆盖初始提示、插队/追加消息、
钩子注入的消息、助手回复、工具结果，以及中止与 panic 后补齐的结果；该调用**不受取消影响**，适合持久化。但存在两个缺口：

1. `Agent` 内置宿主只把消息追加到内存（[`agent/host.rs`](../crates/zach-agent/src/agent/host.rs)），外部拿不到这个时机；
2. 只传 `&Message`：助手这一轮的 `Usage`、结束原因、所用模型，以及各工具的 `ToolOutcome::details` 在组装消息时被丢弃。
   前者是压缩估算 token 的依据，后者是重新加载后重放差异的依据。

事件流无法替代：事件是面向界面的增量，没有完整消息；工具事件按完成顺序到达而消息按源顺序写入；
中止后的普通事件只尽力投递，而中止时补齐的结果恰好落在这个窗口。

### 新契约（命名暂定）

`LoopHost::on_message(&Message)` 替换为携带元数据的提交记录：

```rust
/// 一条写入对话记录的消息及其附带信息
#[non_exhaustive]
pub struct Commit<'a> {
    pub message: &'a Message,
    pub kind: CommitKind<'a>,
}

#[non_exhaustive]
pub enum CommitKind<'a> {
    /// 初始提示、插队/追加消息、钩子注入的消息
    Input,
    /// 模型回复；被中止时为已生成的部分，此时 usage 与 finish_reason 为空
    Response {
        provider: &'a str,
        model_id: &'a str,
        usage: Option<&'a Usage>,
        finish_reason: Option<&'a FinishReason>,
    },
    /// 工具结果，附带各调用给宿主的详情（按 tool_call_id）；中止时未完成的调用以"已中止"错误结果出现在这里
    ToolResults { details: &'a [(String, Value)] },
    /// 运行任务异常（panic）后为悬空工具调用补齐的结果
    Recovery,
}
```

`Agent` 侧新增独立的 `TranscriptSink` trait，经 `AgentBuilder::transcript(...)` / `Agent::set_transcript(...)` 设置：

```rust
#[async_trait]
pub trait TranscriptSink: Send + Sync {
    async fn on_commit(&self, commit: &Commit<'_>);
}
```

约定：

- **有序且被 await**：循环等回调返回后才继续，回调顺序即对话记录顺序。
- **不可取消**：与 `AgentHooks`（中止时丢弃正在等待的钩子）语义不同，因此不放进 `AgentHooks`。
- **不返回错误**：与钩子一致。落盘失败由实现方自行处理（会话层的做法是记录错误并中止当前运行）。

## 二、数据模型

### 文件结构

一个会话一个 JSONL 文件：第一行是文件头，之后每行一个条目。

```jsonl
{"type":"session","format_version":1,"id":"…","created_at":"…","workspace_root":"/repo","forked_from":null}
{"id":1,"parent":null,"ts":"…","kind":"config","model":{"provider":"anthropic","id":"…"},"system_prompt":"…","tools":["read","edit"]}
{"id":2,"parent":1,"ts":"…","kind":"message","origin":"input","message":{"role":"user","content":[…]}}
{"id":3,"parent":2,"ts":"…","kind":"message","origin":"response","message":{…},"usage":{…},"finish_reason":{…},"model":{…}}
{"id":4,"parent":3,"ts":"…","kind":"message","origin":"tool_results","message":{…},"tool_details":{"call_1":{"diff":"…"}}}
```

- **条目 ID** 是会话内单调递增的整数：人可读，测试与样例文件确定，分叉复制到新文件时保持不变。
- **树形结构**：树条目带 `parent`，指向写入时的当前叶子。今天只有一条线，但回退与分叉不需要迁移格式。

### 条目类型

| kind | 类别 | 内容 | 是否进入上下文 |
| :--- | :--- | :--- | :--- |
| `message` | 树 | `Message` 原样序列化；`origin` 取自 `CommitKind`；附带 `usage`、`finish_reason`、`model`、`tool_details` | 是 |
| `compaction` | 树 | `summary`、`first_kept`（保留区起点条目 ID）、`tokens_before`、`read_files`、`modified_files` | 以摘要消息替代被压缩部分 |
| `config` | 树 | 只记录变化的字段：`model`、`reasoning`、`system_prompt`、`tools` | 否（供审计与恢复） |
| `custom` | 树 | `namespace` + 任意 `data`，宿主扩展状态，随分支生效 | 否 |
| `checkout` | 控制 | `target`：把当前叶子移到某个树条目 | 否 |

`config` 记录完整系统提示词（只在变化时写一次），恢复会话时宿主可以复现模型当时看到的内容。系统提示词本身仍由运行时按当前配置生成，不从日志投影。

### 当前叶子

按文件顺序重放即可确定：树条目使叶子变为自身，`checkout` 使叶子变为其 `target`。
叶子因此被显式持久化，回退后重启不会丢失位置（pi 恢复时总取文件最后一行，回退位置会丢失）。

## 三、投影：日志 → 模型上下文

`project(entries, leaf) -> Vec<Message>` 是纯函数，压缩、修复与分支规则都集中在这里，用固定样例做单元测试，不触及 IO。

1. 从叶子沿 `parent` 回溯到根，得到当前分支路径（按时间正序）。
2. 若路径上有 `compaction` 条目，取最后一个 `C`：
   上下文 = 摘要消息 + 路径上从 `C.first_kept` 到 `C` 之前的消息 + `C` 之后的消息。
   摘要消息为用户角色：`以下是此前对话的摘要……<summary>…</summary>`。
3. 其余路径上只取 `message` 条目。
4. **崩溃修复**：任何发起了工具调用、但后面没有覆盖全部调用的工具结果的助手消息，紧随其后补上
   "运行中断，工具未得出结果"的错误结果（与 `zach-agent` 的 panic 善后同一措辞）。修复只发生在投影中，不写回日志；
   因为投影是确定的，模型每次看到的内容一致。

## 四、写入与一致性

- **逐条追加**：每个条目序列化为一行，一次写入，写后 flush。
- **fsync 策略**（`Durability`）：`EveryEntry`、`TurnBoundary`（默认：轮次结束与运行结束时 fsync）、`OsDefault`。
- **懒创建**：第一条 `message` 条目写入时才创建文件，只开会话不提问不会留下空文件。
- **残行处理**：加载时最后一行不完整则忽略并给出诊断；以追加模式打开前先截断到最后一个换行符，
  避免新条目接在残行后面。这是除版本迁移外唯一允许的非追加写。
- **单写者**：同一会话同时只允许一个 `Session` 实例写入；跨进程互斥本期不做，由宿主保证。
- **写入失败**：会话进入失败状态，钩子拒绝执行任何工具并结束当前运行，运行结果报告写入错误，之后拒绝新的运行；不在日志缺失的情况下继续修改工作区。

### 大对象

消息中超过阈值（暂定 32KB）的 `FileData::Data` 写入 `blobs/sha256-<hex>`，条目中替换为
`FileData::Url { url: "zach-blob:sha256-<hex>" }`，加载时还原。blob 先于引用它的条目写入，日志不会指向缺失的 blob；
按内容寻址天然去重。替换在会话层完成，存储只提供读写 blob 的原语。

## 五、存储抽象

当前提供进程内的 `MemoryStore`（测试与不需要持久化的宿主）；下文的 `JsonlStore` 是待实现的默认文件存储规格。

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create(&self, header: &SessionHeader) -> Result<(), StoreError>;
    async fn append(&self, id: &SessionId, entries: &[Entry]) -> Result<(), StoreError>;
    async fn sync(&self, id: &SessionId) -> Result<(), StoreError>;
    async fn load(&self, id: &SessionId) -> Result<StoredSession, StoreError>; // 头、条目、诊断
    async fn list(&self) -> Result<Vec<SessionHeader>, StoreError>;
    async fn put_blob(&self, data: &[u8]) -> Result<BlobRef, StoreError>;
    async fn get_blob(&self, blob: &BlobRef) -> Result<Vec<u8>, StoreError>;
}
```

- `JsonlStore` 的根目录由宿主指定（运行时不假设任何全局目录）：`<root>/sessions/<id>.jsonl`、`<root>/blobs/`。
  Unix 上目录权限 0700、文件 0600。
- 测试使用纯内存的存储替身；`JsonlStore` 自身的用例使用临时目录。
- `StoreError` 区分：不存在、格式损坏（带行号）、版本过新、IO 错误。

## 六、Session：驱动 Agent

```rust
impl Session {
    pub fn create(store, agent: Agent, options: SessionOptions) -> Self;               // 懒创建，不触及存储
    pub async fn open(store, id, agent, options) -> Result<Self, SessionError>;
    pub fn prompt(&self, messages: Vec<Message>) -> Result<SessionRun, SessionError>;  // 后台任务驱动
    pub fn continue_run(&self) -> Result<SessionRun, SessionError>;
    pub fn agent(&self) -> &Agent;   // 插队、追加、中止、审批答复直接操作 Agent，写入时经提交通知落盘
    pub async fn compact(&self) -> Result<Option<EntryId>, SessionError>;   // 手动压缩
    pub async fn rewind(&self, to: EntryId) -> Result<(), SessionError>;    // 追加 checkout
    pub async fn fork(&self, at: EntryId, agent: Agent, options: SessionOptions)
        -> Result<Session, SessionError>;                                    // 以根到 at 的分支新建会话
    pub async fn context(&self) -> Result<Vec<Message>, SessionError>;      // 当前投影，大对象已还原
    pub fn entries(&self) -> Vec<Entry>;                                     // 供宿主展示树
    pub async fn append_custom(&self, namespace, data: Value) -> Result<EntryId, SessionError>;
}
```

一次 `prompt` 的流程：

1. 估算当前上下文，超过阈值则先压缩（见下节）。
2. `agent.set_messages(project(...))`，再 `agent.prompt(messages)`。
3. 运行中，会话实现的 `TranscriptSink` 把每次提交追加为 `message` 条目，叶子随之前进。
4. 轮次之间，会话实现的 `AgentHooks::prepare_next_turn` 检查阈值，需要时压缩并把 `request.context.messages` 换成新投影。
   会话需要占用 `Agent` 的钩子位，宿主自己的钩子通过 `SessionOptions` 传入、由会话层串联调用。
5. 运行以 `ContextLengthExceeded` 失败时：压缩（原因记为溢出）后 `continue_run()`，每次 `prompt` 最多一次。
   `zach-agent` 不把失败的那一轮写入对话记录，所以这里不需要 pi 的"撤销失败尝试"条目。

`SessionRun` 是 `Stream<Item = SessionEvent>`：`Agent(AgentEvent)` 原样转发，另有 `Compaction { started / finished / failed }`
与 `PersistFailed`。溢出重试时内部换接新的 `AgentRun`，对使用方是同一个流。

`Agent` 内部的对话记录在运行中途压缩后会与投影不一致，这是预期行为：每次运行开始都以投影覆盖，会话是唯一事实来源。

## 七、上下文压缩

### 触发

- 窗口：`model.profile()` 的 `max_input_tokens`，否则 `context_window`；都没有时取 `SessionOptions` 的配置；仍没有则只支持手动压缩。
- 估算：分支上最后一条带 `usage` 的回复的输入 + 输出 token，加上其后各消息的字符数 / 4；没有用量时整体按字符估算。
- 阈值：估算值 > 窗口 − `reserve_tokens`（默认 16384）。
- 时机：`prompt` 开始前、轮次之间、溢出失败后、手动调用。

### 切点

从叶子向前累加估算 token，直到达到 `keep_recent_tokens`（默认 20000），在不早于该处的最近一个合法切点切分。
合法切点只有**用户消息或助手消息**的开头：绝不把工具调用与其结果分开。保留区若从轮次中途开始（切点是助手消息），
该轮之前的部分随其他被压缩的消息一起进入摘要（不单独生成"轮内前情"摘要）。
可见消息总量不足保留量时没有可压缩的内容；保留量不超过窗口的一半，避免小窗口模型永远无内容可压缩。

### 摘要

- 由 `Summarizer` trait 生成，默认实现用当前模型发一次独立请求；宿主可以换成更便宜的模型。
- 输入：被压缩部分序列化为纯文本（工具结果截断到 2000 字符、丢弃思考链）+ 上一次摘要（如有，要求"更新"而非重写）。
- 输出模板：目标 / 约束与偏好 / 进展（已完成、进行中、受阻）/ 关键决定 / 下一步 / 关键上下文。
- 文件追踪：从被压缩部分的工具调用入参提取读过与改过的路径（工具名可配置，默认 `read` → 读，`write`、`edit` → 改），
  与上一次压缩的列表合并后写入条目，并附在摘要末尾。
- 失败处理：预防性压缩失败则照常继续并报告；溢出后的压缩失败则以原错误结束运行。

## 八、格式演进

- 文件头的 `format_version` 从 1 开始；加载器支持 `1..=CURRENT`，更高版本拒绝加载（不猜测未知语义）。
- 同一版本内出现未知 `kind` 视为损坏：可能影响上下文的新条目类型必须伴随版本号提升。
- 打开旧版本文件时先迁移：写入临时文件后原子替换，保留 `.bak`。这是除残行截断外唯一的改写。
- **`Message` 的 serde 形态自此成为对外契约**。仓库内提交固定的会话样例文件
  （`crates/zach-coding-agent/tests/fixtures/sessions/v1/*.jsonl`），CI 验证它们能加载且投影结果不变；
  zach-ai-core 中任何破坏序列化的改动都会在这里失败，必须随之提升版本并编写迁移。旧版本样例永久保留。

## 九、安全

- 不写入任何凭据；工具输出中可能出现的敏感内容由宿主负责（例如在提交通知中脱敏后再交给会话层）。
- 文件与目录权限见存储一节。

## 十、与 pi 的差异

| 方面 | pi | 本设计 |
| :--- | :--- | :--- |
| 失败尝试 | 以 `context_edit` 条目撤销 | 失败轮次本就不写入，无需撤销条目 |
| 当前叶子 | 不持久化，恢复时取最后一行 | `checkout` 条目显式持久化 |
| 条目 ID | 随机 8 位十六进制 | 会话内递增整数，样例文件确定 |
| 系统提示词 | 以差量消息存入对话记录 | 运行时生成，`config` 条目仅记录变化供审计 |
| 大对象 | 内联 | 按内容寻址的 blob |
| 工具详情 | 存于工具结果消息 | 存于条目元数据，`Message` 保持不变 |

## 实施状态

| 步骤 | 提交 | 状态 |
| :--- | :--- | :--- |
| `zach-agent`：`Commit` / `CommitKind`、`LoopHost::on_commit`、`TranscriptSink` | `e464210` | 已完成 |
| 会话模型与投影、v1 样例文件 | `e374222` | 已完成 |
| 存储抽象与 `MemoryStore` | `d332314` | 已完成；**`JsonlStore` 待实现** |
| `Session`：运行、逐条落盘、大对象、回退与分叉、写入失败处理 | `687bad7` | 已完成 |
| 压缩：估算、切点、摘要、文件追踪、轮间压缩与溢出重试 | `4fa3cf5` | 已完成 |

## 已确认的决定

1. 会话层放在 `zach-coding-agent` 的 `session` 模块，出现第二个使用方时再独立成 crate。
2. 默认刷盘策略为 `TurnBoundary`。
3. 摘要默认使用会话当前的模型，可经 `ModelSummarizer::with_model` 或自定义 `Summarizer` 替换。
4. 跨进程互斥本期不做，由宿主保证同一会话只有一个写入方。
