# 语言模型中间形态（Rust）

本文是 Zach 自己的厂商无关 IR，语义对齐 [LanguageModelV4](./language-model-v4-intermediate-format.md)，落地在 `zach-ai-core::language_model`。

V4 解决的问题仍然成立：上层 Agent 只认识一套消息 / 工具 / 流式事件，具体厂商协议关在 `zach-ai` 适配器里。Rust 版不追求和 TypeScript SDK 的 JSON 逐字段兼容，而是把同一套语义收成能编译期约束的类型。

## 1. 分层

```
zach-agent          会话、工具执行、审批、循环
        │
        ▼
zach-ai-core        中间形态 + LanguageModel trait   ← 本文
        │
        ▼
zach-ai             OpenAI / Anthropic / … 适配器
```

- **Prompt IR**：发给模型之前的标准化消息，不是用户 UI 消息。
- **Call IR**：一次调用的采样、工具、输出格式等参数。
- **Result / Stream IR**：厂商响应映射后的统一产物。
- **Trait**：厂商适配器只实现 `generate` / `stream`。

规范版本是 `v1`。它表示 Zach 这套 IR 的版本，不是「我们实现了 TS 的 v4」。语义对照见第 6 节。

## 2. 相对 V4 的 Rust 化取舍

| 点 | V4 (TS) | Zach (Rust) | 原因 |
| --- | --- | --- | --- |
| 命名 | `LanguageModelV4Xxx` 铺开 | 模块 `language_model` + 短类型名 | crate 已是命名空间 |
| 可空 | `T \| undefined`、`unknown` | `Option<T>`、`serde_json::Value` | 所有权清晰，避免 `unknown` |
| 角色内容 | 交叉类型 + 宽联合 | `Message` 按 role 拆开，content 各用窄枚举 | 系统消息塞文件会直接编不过 |
| 文件字节 | `Uint8Array \| string` | `FileBytes::{Bytes, Base64}` | 内存里用字节，JSON 仍可走 base64 |
| URL | `URL` 对象 | `String` | core 不引入 `url` crate |
| Schema | `JSONSchema7` | `JsonSchema = Value` | 只透传，不在 IR 层校验 |
| 取消 | `AbortSignal` | 不放进 `CallOptions` | 取消靠 drop future / `select!` |
| 流 | `ReadableStream` | `Pin<Box<dyn Stream<Item = StreamPart> + Send>>` | 标准 futures |
| 时间 | `Date` | RFC 3339 `String` | core 不引入 chrono |
| 原生 URL 能力 | `Record<string, RegExp[]>` | `BTreeMap<String, Vec<String>>` | 正则以源字符串保存，适配器编译 |
| 错误 | 流里 `error: unknown` | trait 返回 `LanguageModelError`；流内错误用 `Value` | 调用失败和流内错误分开 |
| 厂商扩展 | `Record<string, JSONObject>` | newtype `ProviderBag` | 后续可加取值辅助，不泄漏 HashMap |

刻意保持和 V4 一样的不对称：

- **Prompt 侧 tool-call.input** 是结构化 `Value`（回放历史时已经是对象）。
- **输出侧 tool-call.input** 是字符串化 JSON（流式拼参、厂商原始文本）。
- **生成文件 / 推理文件** 只允许 `data` / `url`，没有 `reference` / `text`。
- **工具结果** 在 Prompt 里是 `ToolResultOutput` 联合；在 Generate 里是「厂商已执行」的 `Value + is_error + preliminary`。

这两套不要合成一种类型。合成之后适配器就要猜「这段 JSON 是历史回放还是厂商原始输出」。

## 3. 模块

```
zach-ai-core/src/language_model/
  shared.rs     厂商袋、文件、警告、头
  prompt.rs     Message / Part
  tool.rs       工具定义、选择、Prompt 侧工具结果
  call.rs       CallOptions、输出格式、推理档位
  generate.rs   非流式结果、输出 Content、用量、结束原因
  stream.rs     流式结果与事件
  model.rs      LanguageModel trait
  error.rs      调用级错误
```

上层只依赖 `zach-ai-core`。`zach-ai` 把厂商协议 ↦ 这套 IR；`zach-agent` 把这套 IR ↦ 会话状态。

## 4. 核心类型（逻辑视图）

### 4.1 厂商袋

```text
ProviderBag = BTreeMap<厂商名, JSON Object>
```

入参叫 `provider_options`，出参叫 `provider_metadata`，结构相同、方向不同。不要用同一个字段名混用。

文件引用 `ProviderReference` 是 `BTreeMap<厂商名, 文件 ID>`，禁止带 `type` 字段，以免和带 tag 的 `FileData` 撞车。

### 4.2 消息

```text
Prompt = [Message]

Message =
  | system    { content: String }
  | user      { content: [TextPart | FilePart] }
  | assistant { content: [Text | File | Custom | Reasoning | ReasoningFile | ToolCall | ToolResult] }
  | tool      { content: [ToolResult | ToolApprovalResponse] }
```

每条消息、每个 part 都可以挂自己的 `provider_options`。

`CustomPart.kind` 必须是 `{provider}.{provider-type}`，用 newtype 校验，不要裸 `String`。

### 4.3 工具定义与延迟加载

```text
ToolDefinition =
  | Function(FunctionTool) // 完整客户端函数工具（带 input_schema，支持可选 defer_loading）
  | Provider(ProviderTool) // 厂商云端原生工具（如 OpenAI web_search、Anthropic bash）

FunctionTool {
  name, description, input_schema,
  strict?, defer_loading?, input_examples?, provider_options?
}

ProviderTool {
  id,   // `<provider-id>.<unique-tool-name>`
  name,
  args  // 厂商约定配置参数
}

ToolRegistry & ToolResolver (zach-agent)
  ToolResolver 属于 Agent 编排层运行时契约，负责按需动态拉取解析工具定义并注入注册表；
  ToolRegistry 负责管理本地活跃工具、轻量元数据发现以及执行调度。
```

### 4.4 调用

```text
CallOptions {
  prompt,
  采样: max_output_tokens / temperature / top_p / top_k / penalties / seed / stop_sequences
  response_format: text | json { schema?, name?, description? }
  tools, tool_choice
  reasoning: provider-default | none | minimal | low | medium | high | xhigh
  include_raw_chunks
  headers            // 仅 HTTP 厂商
  provider_options
}
```

没有 `abort_signal`。需要取消时，调用方 drop `generate`/`stream` 的 future，或在外层 `tokio::select!`。适配器里对 reqwest 等请求也要跟着 future 取消走。

### 4.5 输出 Content

```text
Content =
  | text | reasoning | custom
  | file | reasoning-file
  | tool-approval-request
  | source (url | document)
  | tool-call          // input: String
  | tool-result        // 厂商已执行；可 preliminary
```

`FinishReason` 分 `unified`（跨厂商）和 `raw`（厂商原值）。`Usage` 拆 input（total / no_cache / cache_read / cache_write）和 output（total / text / reasoning），并保留 `raw`。

### 4.6 流

流事件按「一段内容一条生命周期」切：

```text
text/reasoning/tool-input:  *-start → *-delta* → *-end
一次性对象:                 tool-call / tool-result / file / source / custom / approval
控制面:                     stream-start / response-metadata / finish / raw / error
```

`stream()` 只表示「流建立成功」。建连失败走 `Result::Err`；生成到一半的模型错误走 `StreamPart::Error`，这样已经吐出的 delta 不会被一次 `Err` 丢掉。

## 5. Trait

```rust
#[async_trait]
pub trait LanguageModel: Send + Sync {
    fn specification_version(&self) -> SpecificationVersion; // V1
    fn provider(&self) -> &str;
    fn model_id(&self) -> &str;
    fn supported_urls(&self) -> SupportedUrls;

    async fn generate(&self, options: CallOptions) -> Result<GenerateResult, LanguageModelError>;
    async fn stream(&self, options: CallOptions) -> Result<StreamResult, LanguageModelError>;
}
```

`supported_urls` 做成同步方法：key 是媒体类型（`image/*`、`application/pdf`），value 是正则源字符串。需要异步探测的厂商在构造适配器时查好，不要把 Promise 语义带进 trait。

## 6. 与 V4 字段对照

| V4 | Zach |
| --- | --- |
| `LanguageModelV4` | `LanguageModel` |
| `specificationVersion: 'v4'` | `SpecificationVersion::V1` |
| `doGenerate` / `doStream` | `generate` / `stream` |
| `LanguageModelV4CallOptions` | `CallOptions` |
| `LanguageModelV4Prompt` | `Prompt` |
| `LanguageModelV4Message` | `Message` |
| `SharedV4*` | `ProviderBag` / `FileData` / `Warning` / `Headers` |
| `LanguageModelV4Content` | `Content` |
| `LanguageModelV4StreamPart` | `StreamPart` |
| `abortSignal` | （不进入 IR） |
| `JSONValue` / `unknown` | `serde_json::Value` |
| `Date` | RFC 3339 `String` |

JSON 序列化用 **snake_case 字段** + **kebab-case 的 `type`/`role` tag**（`tool-call`、`content-filter`）。这是给日志、录制回放、测试夹具用的，不是 OpenAI 线协议。

## 7. 适配器约定

1. **只在边界转换一次**。`zach-ai` 进 IR，`zach-agent` 出 IR，中间不要再出现厂商 DTO。
2. **不认识的厂商字段**进 `provider_options` / `provider_metadata` / `CustomPart`，不要为此改 IR。
3. **不支持的采样项**不要静默丢；写入 `Warning::Unsupported`。
4. **流必须能拼回非流式结果**：同一 `id` 的 start/delta/end 拼起来，应等于 `generate().content` 里对应块。
5. **tool-call id** 在同一次调用内唯一，且与后续 tool-result / approval 对齐。

## 8. 先不做

- JSON Schema 在 IR 层校验（那是工具执行器的事）。
- 把 Prompt 侧和 Generate 侧的 tool-call/tool-result 合成一种。
- 在 core 里引入 HTTP、regex、chrono、具体厂商 SDK。
- 会话、记忆、多 Agent 协议（属于 `zach-agent`）。
