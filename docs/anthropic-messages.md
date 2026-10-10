# Anthropic Messages API

`zach-ai` 提供 `AnthropicMessagesModel`，实现 `zach_ai_core::LanguageModel`。请求发送到
`{base_url}/v1/messages`，默认根地址为 `https://api.anthropic.com`（注意不含 `/v1`），
认证使用 `x-api-key` 头（Key 为空时不发送，用于无需鉴权的网关），
并固定携带 `anthropic-version: 2023-06-01`。

```toml
[dependencies]
zach-ai = { path = "crates/zach-ai", default-features = false, features = ["anthropic", "rustls-tls"] }
zach-ai-core = { path = "crates/zach-ai-core" }
```

```rust
use futures::StreamExt;
use zach_ai::AnthropicMessagesModel;
use zach_ai_core::{CallOptions, LanguageModel, Message, StreamPart};

let model = AnthropicMessagesModel::new(api_key, "claude-sonnet-4-5");
let mut stream = model.do_stream(CallOptions::new(vec![Message::user("你好")])).await?;
while let Some(part) = stream.next().await {
    if let StreamPart::TextDelta { delta, .. } = part? { print!("{delta}"); }
}
```

## 支持范围

| 通用能力 | Messages 映射 |
| --- | --- |
| 系统提示 | 开头的 `System` 消息提升为顶层 `system` 块；出现在其他位置会在请求前报错 |
| 文本、图片、PDF | `text` / `image` / `document` 块，支持 Base64、URL 与 `anthropic` 文件引用 |
| 内联文本文档 | `FileData::Text` 或 `text/*` 字节映射为 `document` 的 `text` 源，`filename` 作为标题 |
| 函数工具 | `tools[].name/description/input_schema`，透传 `strict`、`defer_loading`、`input_examples` |
| 服务端工具 | `ProviderTool { id: "anthropic.<name>", args }`，`args` 必须包含版本化 `type` |
| 工具选择 | `auto` / `none` / `any` / `tool` |
| 工具结果 | 紧随其后的 `user` 消息中的 `tool_result`；错误与拒绝设置 `is_error` |
| JSON Schema 输出 | `output_config.format.type = json_schema`；无 Schema 的 JSON 模式不支持 |
| 采样参数 | `temperature`、`top_p`、`top_k`、`stop_sequences`；惩罚项与 `seed` 不支持 |
| 推理档位 | 见下文 |
| 提示缓存 | 任意内容块、系统提示与工具定义的 `provider_options.anthropic.cache_control` |

`max_tokens` 是 Messages API 的必填项：未设置 `max_output_tokens` 时取模型档案的
`max_output_tokens`，未知模型使用 8192。档案默认来自内置目录，自定义端点或目录未收录的
模型可用 `with_profile(ModelProfile)` 注入，注入值优先。

同角色相邻消息会合并为一条（例如工具结果消息之后紧跟的用户消息），以满足
user/assistant 交替的要求；末尾助手预填充文本会去掉结尾空白。

## 推理（思考）

通用 `reasoning` 档位按模型代际（从 id 中的版本号识别，如 `claude-opus-4-6`）映射：

| `ReasoningEffort` | Claude 4.6 及之后（含代际未知的别名） | Claude 4.5 及之前 |
| --- | --- | --- |
| `ProviderDefault` | 不设置 `thinking` | 不设置 `thinking` |
| `None` | `thinking: {"type": "disabled"}` | 同左 |
| `Minimal` / `Low` | `thinking: {"type": "adaptive", "display": "summarized"}` + `effort: low` | `enabled`，`budget_tokens` 1024 / 4096 |
| `Medium` / `High` / `Xhigh` / `Max` | 同上，`effort` 对应 `medium` / `high` / `xhigh` / `max` | `enabled`，`budget_tokens` 16384 / 32768 / 65536 / 131072 |

旧代际的 `budget_tokens` 会压到 `max_tokens - 1` 以下；`max_tokens` 不足 1025 时无法启用思考，
发送前报错。显式配置 `provider_options.anthropic.thinking` 会整体替换上述默认值。

思考开启（`enabled` / `adaptive`）时 Anthropic 不接受 `temperature` 与 `top_k`，这两个参数会被
丢弃并以 `Compatibility` 警告透出；`top_p` 保留（API 允许 0.95 ~ 1）。

思考块以 `Reasoning` 事件透出，签名随 `ReasoningEnd` 的 `provider_metadata.anthropic.signature`
给出；`redacted_thinking` 是正文为空、只带 `anthropic.redacted_thinking` 元数据的推理块。
回放时两者分别还原为 `thinking` 与 `redacted_thinking` 块，没有签名的推理块不会发送。
Anthropic 把思考 token 计入 `output_tokens`，适配器不单独报告推理 token 数量。

注意 Anthropic 的限制：**思考与强制工具选择不能共存**。`tool_choice` 为 `any` / `tool` 时，
`enabled` 模式会被 API 以 400 拒绝，`adaptive` 模式虽被接受但该轮不会产生思考块。
需要推理回放的多轮场景应使用 `tool_choice: auto`（或 `none`），由提示词引导工具调用。
适配器不会静默改写这两个参数。

## 档案校验与警告

请求构建时会对照模型档案（内置目录或 `with_profile` 注入）校验通用参数，档案未知时跳过：

- 模型不支持推理却设置了档位：丢弃并给出 `Unsupported` 警告；
- 档案未声明可关闭推理却要求 `ReasoningEffort::None`：不发送关闭指令并给出 `Compatibility` 警告；
- 档位不在模型支持列表里：`Minimal` 降级为 `low` 并给出 `Compatibility` 警告，其余发送前报 `UnsupportedFeature`；
- 档案声明不接受 `temperature`：丢弃并给出 `Unsupported` 警告。

警告随流式的 `StreamStart.warnings` 与非流式的 `GenerateResult.warnings` 返回，
Agent 层会以 `model.warnings` 事件呈现。

## HTTP 配置

`with_client` 可传入自定义 `reqwest::Client`；默认客户端设置 30 秒连接超时与 300 秒读取超时
（相邻两次收到字节的间隔），不设整体超时。

## 厂商扩展

`provider_options.anthropic` 支持以下键：

- `thinking`、`output_config`、`metadata`、`service_tier`、`context_management`、
  `mcp_servers`、`container`：写入请求正文（`output_config` 与 `metadata` 按字段合并）。
- `betas`：字符串数组，与默认请求头中的 `anthropic-beta` 合并后发送。

其他键会在请求前以 `UnsupportedFeature` 拒绝，避免静默改变适配器行为。

## 响应映射

- 用量：`input_tokens` 为未缓存部分，总量为其与 `cache_read_input_tokens`、
  `cache_creation_input_tokens` 之和，后两者分别映射为 `cache_read` / `cache_write`。
- 结束原因：`end_turn` / `stop_sequence` → `stop`，`max_tokens` → `length`，`tool_use` →
  `tool_calls`，`refusal` → `content_filter`，`pause_turn` 等 → `other`；原始值保留在 `raw`，
  `stop_sequence` 与 `stop_details`（拒绝类别与说明）在 `provider_metadata.anthropic` 中。
- 服务端工具：`server_tool_use` / `mcp_tool_use` 以 `provider_executed = true` 的工具调用透出，
  `*_tool_result` 块以 `ToolResult` 透出；两者都在元数据中保留原始块，下一轮按原样回放。
- 引用：文本块中的 `citations` 以 `Source` 事件透出（网页检索为 URL，文档定位为 Document）。
- 未知内容块以 `Custom { kind: "anthropic_content_block" }` 透出并原样回放。

流式解析按 `content_block_*` 事件维护生命周期，`message_delta` 给出累计用量与结束原因，
`message_stop` 之后不再读取网络；在此之前连接断开视为传输错误，流内 `error` 事件
（如 `overloaded_error`）使本轮结果为 `Error`。HTTP 429 与 529 映射为可重试的限流错误。

## 联调

```powershell
cargo xtask probe anthropic           # 全套：generate、stream 用 mixed.json，agent 用 anthropic-tools.json
cargo xtask probe anthropic stream    # 只跑流式混合场景
```

连接配置读取 `PROBE_ANTHROPIC_*`（缺失时回退 `PROBE_*`）；根地址默认 `https://api.anthropic.com`，
不应包含 `/v1/messages` 后缀。内置混合场景 `mixed.json` 可直接复用，Agent 场景使用 Anthropic
专用的 `anthropic-tools.json`（`cargo xtask probe anthropic agent`）：

`responses-tools.json` 第一轮强制 `tool_choice: add`，按上文限制该轮不会思考，`replay: reasoning`
断言会按设计判失败；此外联调中观察到 JSON Schema 输出与工具叠加时，模型的思考虽决定调用
工具、最终却被 Schema 约束直接产出 JSON。专用场景因此改为 `tool_choice: auto` 且不带 Schema，
在 `claude-sonnet-4-6` 上可稳定通过全部 16 项断言（含思考签名回放与工具结果回放）。

### 安全分类器拒绝（`refusal`）

Fable 5 / 5.1、Opus 5 / 5.5、Sonnet 5.5、Haiku 5.5 内置安全分类器，可能以 HTTP 200 +
`stop_reason: refusal` 拒绝请求（官方文档明确"正常工作也可能触发"）。适配器映射为
`content_filter`，拒绝类别与说明在 `provider_metadata.anthropic.stop_details`。
分类器是整体性判断：联调中 `claude-opus-5` 对"图片 + 按字段输出 JSON + system 提示"
稳定返回 `reasoning_extraction`，在通过的组合上仅追加一个 Schema 或一句话就会翻转；
`claude-fable-5` 偶发 `cyber`（思考数 token 后中途拒绝）。因此**不要靠改措辞或附件迁就
分类器**，Anthropic 的官方做法是回退到另一模型重试：

- 服务端：`provider_options.anthropic` 传 `fallbacks: "default"` 并在 `betas` 加
  `server-side-fallback-2026-07-01`（仅 Claude API 直连；Vertex / Bedrock / Foundry 转发的
  网关会静默忽略）。
- 客户端：收到 `content_filter` 且 `stop_details` 非空时，由调用方换模型重发同一请求。

内置场景在本次联调网关上的实测（`stream mixed.json` / `agent anthropic-tools.json`）：

| 模型 | 流式混合场景 | Agent 场景 | 备注 |
| --- | --- | --- | --- |
| `claude-fable-5-1` | ✅ | ✅ 2/2 | 最新模型，推荐联调默认 |
| `claude-fable-5` | ✅ | ⚠ 1/2 | 偶发 `cyber` 误报 |
| `claude-opus-5` | ❌ `reasoning_extraction` | ❌ | 对本场景稳定误报 |
| `claude-opus-4-8` | — | ✅ 2/2 | 4.x 无分类器 |
| `claude-sonnet-4-6` | ✅ | ✅ 3/3 | |
