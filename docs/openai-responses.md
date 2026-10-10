# OpenAI Responses API

`zach-ai` 提供 `OpenAiResponsesModel`，实现 `zach_ai_core::LanguageModel`。
请求发送到 `{base_url}/responses`，默认根地址为 `https://api.openai.com/v1`。
此适配器使用 Responses 协议；仅提供 Chat Completions 的兼容端点不能使用它。

仓库内的真实联调入口为 `cargo xtask probe responses`（全套）或 `cargo xtask probe responses stream`。
凭据配置、普通请求及 Agent 工具调用的验证方法见 [真实 API 联调说明](probe.md)。

## 安装与调用

默认 feature 已包含 `openai` 和 `rustls-tls`。只启用 Responses 时可以写：

```toml
[dependencies]
zach-ai = { path = "crates/zach-ai", default-features = false, features = ["openai-responses", "rustls-tls"] }
zach-ai-core = { path = "crates/zach-ai-core" }
futures = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

`openai` 当前是 `openai-responses` 的上层开关。`native-tls` 可以替代 `rustls-tls`；
不启用适配 feature 时仍可使用模型档案目录。模型 ID 与凭据由调用方传入。

```rust
use futures::StreamExt;
use zach_ai::OpenAiResponsesModel;
use zach_ai_core::{CallOptions, LanguageModel, Message, ModelError, StreamPart};

async fn chat(api_key: String, model_id: String) -> Result<(), ModelError> {
    let model = OpenAiResponsesModel::new(api_key, model_id);
    let options = CallOptions::new(vec![Message::user("你好")]);
    let mut stream = model.do_stream(options).await?;
    while let Some(part) = stream.next().await {
        match part? {
            StreamPart::TextDelta { delta, .. } => print!("{delta}"),
            StreamPart::Error { message, raw } => {
                return Err(ModelError::provider_error("openai", message, raw));
            }
            _ => {}
        }
    }
    Ok(())
}
```

一次性生成使用 `model.do_generate(options).await?`，结果可调用 `text()`。
接入现有 Agent 时，在 README 的工具示例中将 `model` 参数换为
`Arc::new(OpenAiResponsesModel::new(api_key, model_id))` 即可，工具和事件消费逻辑沿用原接口。

## 支持范围

| 功能 | 行为 |
| --- | --- |
| 普通生成与 SSE | 文本、拒绝、推理摘要、函数参数增量及最终工具调用 |
| 多轮函数调用 | 区分输出项 `id` 和执行关联 `call_id`，按原始顺序回放调用与结果 |
| 文本历史 | 带 `item_id` 的助手文本以原始 message 项回放并合并相邻分段，满足加密推理的 id 配对要求 |
| 推理历史 | 保留原始推理项及 `encrypted_content`，通过 provider metadata 回放 |
| 推理正文 | 多段摘要以空行分隔；没有摘要的推理项（如 gpt-oss 的 `reasoning_text`）取其原始正文 |
| 附件 | 图片、PDF 的 URL/字节/`openai` 文件引用，以及内联文本 |
| 工具结果 | 文本、JSON、错误、拒绝和受支持的复合内容块 |
| 输出格式 | 文本、JSON Object、JSON Schema |
| 统计 | 输入、缓存读取、输出和推理 token；响应 ID、模型 ID、时间戳 |
| 引用 | URL 与文件引用转换为 `SourceContent` |
| 配置 | 根地址、HTTP 客户端、请求头、采样参数、输出上限、推理档位 |

暂未支持服务端内置工具（如 Web Search、MCP、Code Interpreter）、后台任务、
`previous_response_id` 续接、音视频和生成图片。这些能力需要单独适配其执行与回放语义。
声明不支持的工具或请求参数会在发送前返回 `UnsupportedFeature`。
通用参数 `top_k`、`presence_penalty`、`frequency_penalty`、`stop_sequences`、`seed` 不映射到 Responses。

函数声明未指定 `strict` 时发送 `false`，JSON Schema 输出未指定 `strict` 时不发送该字段，
都保留通用 JSON Schema 的可选字段语义。显式启用严格模式（`FunctionTool::with_strict`、
`ResponseFormat::with_strict`）时，调用方需提供满足 OpenAI 严格模式要求的 schema
（每个对象 `additionalProperties: false` 且全部字段 required）；适配器不自动修改 schema。
模型的具体能力仍取决于所选模型和端点。

## 档案校验与警告

请求构建时会对照模型档案（内置目录或 `with_profile` 注入）校验通用参数，档案未知时跳过：

- 模型不支持推理却设置了档位：丢弃并给出 `Unsupported` 警告；
- 档案未声明可关闭推理却要求 `ReasoningEffort::None`：不发送关闭指令并给出 `Compatibility` 警告；
- 档位不在模型支持列表里：有降级别名时降级并给出 `Compatibility` 警告，否则发送前报 `UnsupportedFeature`；
  本适配器的别名为 `Max` → `Xhigh`（`max` 仅 gpt-5.6 及更新代际的档案声明，档案未知时原样发送）；
- 档案声明不接受 `temperature`：丢弃并给出 `Unsupported` 警告。

警告随流式的 `StreamStart.warnings` 与非流式的 `GenerateResult.warnings` 返回，
Agent 层会以 `model.warnings` 事件呈现。

## 历史、推理与扩展参数

默认 `store: false`，并请求 `include: ["reasoning.encrypted_content"]`，方便 Agent 自行维护完整历史。
可以将 `GenerateResult::into_assistant_message()` 追加到下一轮 Prompt。
保留返回的 provider metadata，避免丢失加密推理；没有 Responses 回放数据的外部推理摘要会被忽略，
不会作为助手正文发送。

可通过 `CallOptions.provider_options` 的 `openai` 对象传入以下参数：

- `reasoning`、`text`、`parallel_tool_calls`；
- `metadata`、`service_tier`、`truncation`、`store`、`include`；
- `user`、`safety_identifier`、`prompt_cache_key`、`prompt_cache_retention`。

对象字段合并，同名字段由扩展参数覆盖；`include` 始终保留加密推理选项。
例如请求可见推理摘要：

```rust
use zach_ai_core::{CallOptions, ProviderOptions, ReasoningEffort};

let mut provider = ProviderOptions::new();
provider.insert("openai", serde_json::json!({
    "reasoning": {"summary": "auto"}
}));
let mut options = CallOptions::default().with_reasoning(ReasoningEffort::High);
options.provider_options = Some(provider);
```

该片段需额外声明 `serde_json` 依赖。公共采样配置与扩展选项均会发送到服务端，
具体支持值由所选模型决定；档案信息用于查询，不据此拒绝未知模型。

## HTTP 配置与错误

`with_base_url` 接收包含版本路径的根地址，不包含 `/responses` 后缀。
`with_header` 设置默认头，`CallOptions.headers` 按调用覆盖。适配器内部不启动后台重试。

默认客户端只设 30 秒连接超时；读取与整体超时按请求类型区分：

- 非流式请求（`do_generate`）在服务端生成完之前收不到任何字节，默认整体超时 10 分钟，
  `with_generate_timeout(Some(..))` 可放宽，`None` 表示不限制；
- 流式请求（`do_stream`）等待响应头、以及相邻两次收到数据之间默认最多 5 分钟，
  `with_stream_idle_timeout` 可调整；超时以可重试的 `StreamError` 结束流。

`with_client` 可传入自定义 `reqwest::Client` 配置连接池与代理；客户端级的 `timeout` /
`read_timeout` 会与上述规则叠加，注意 `read_timeout` 同样会约束非流式请求等待响应头的时间。

`with_profile` 注入模型档案，优先于内置目录中的同名条目，用于代理端点或目录未收录的模型。
API Key 为空字符串时不发送 `Authorization` 头，用于接入无需鉴权的网关。

HTTP 401/403 映射为 `Authentication`，429 映射为 `RateLimit`，408 与 5xx 映射为保留状态码
与原始正文的 `ServerError`，其他非成功状态映射为保留原始错误 JSON 的 `ProviderError`。
传输失败映射为 `StreamError` 并保留根因。沿用 core 的重试规则：限流、服务端暂时性故障
和传输错误可由 Agent 重试，`ProviderError` 是对请求本身的拒绝，不重试。

SSE JSON/协议错误产生 `StreamPart::Error`；之后可以继续收集诊断与 usage，
但聚合器不会把本次调用重新判为成功。`*.done` 快照与已收到的增量对不上（中间代理改写了文本）
不算协议错误：保留流式正文，最终快照放在该块 End 事件的 `provider_metadata.openai.final_snapshot`。传输错误产生一次 `Err` 后流立即结束。
没有收到 Responses 终止事件就结束连接时，也会报告传输错误。
设置 `include_raw_chunks = true` 可额外观察原始 SSE JSON 事件。
流按消费需求拉取，丢弃流会释放源流；Agent 的取消机制可以终止正在等待的网络调用。

## 验证与协议依据

测试使用内存字节流和固定 JSON，不访问外部 API、不需要密钥。覆盖 UTF-8 跨块、
CR/LF、多行 SSE、交错工具调用、重复快照、推理回放、普通/流式结果一致性和错误收尾。
发布前的真实端点联调仍需由调用方提供凭据及可用模型。

- [OpenAI 官方流式响应说明](https://developers.openai.com/api/docs/guides/streaming-responses)
- [OpenAI 官方函数调用说明](https://developers.openai.com/api/docs/guides/function-calling)
- [OpenAI 官方推理历史说明](https://developers.openai.com/api/docs/guides/reasoning)
- [OpenAI 官方文件输入说明](https://developers.openai.com/api/docs/guides/file-inputs)
