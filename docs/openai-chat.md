# OpenAI Chat Completions API

`zach-ai` 提供 `OpenAiChatCompletionsModel`，实现
`zach_ai_core::LanguageModel`。请求发送到 `{base_url}/chat/completions`，默认根地址为
`https://api.openai.com/v1`。

```toml
[dependencies]
zach-ai = { path = "crates/zach-ai", default-features = false, features = ["openai-chat", "rustls-tls"] }
zach-ai-core = { path = "crates/zach-ai-core" }
```

```rust
use futures::StreamExt;
use zach_ai::OpenAiChatCompletionsModel;
use zach_ai_core::{CallOptions, LanguageModel, Message, StreamPart};

let model = OpenAiChatCompletionsModel::new(api_key, "gpt-4o");
let mut stream = model.do_stream(CallOptions::new(vec![Message::user("你好")])).await?;
while let Some(part) = stream.next().await {
    if let StreamPart::TextDelta { delta, .. } = part? { print!("{delta}"); }
}
```

适配器支持文本和图片输入、函数工具、JSON Object/JSON Schema 输出、停止词、采样参数、
推理强度、非流式生成与 SSE 流式生成。流式请求自动设置 `stream_options.include_usage`，
以便在末尾返回完整用量。工具调用参数会按增量事件拼接，并在结束时发出完整调用。

接入 Ollama、vLLM、LM Studio 等无需鉴权的兼容端点时 API Key 传空字符串即可，
此时不发送 `Authorization` 头。兼容端点上的模型不在内置目录中，可用
`with_profile(ModelProfile)` 注入档案（例如从 `ModelCatalog::builtin().get("deepseek", ...)` 复制）。

PDF 输入需要使用已上传的 OpenAI `file_id` 或内联 Base64 数据；Chat Completions 不接受
PDF URL。服务端托管工具、音视频和助手历史中的原始推理块暂不在此适配器中回放。

DeepSeek、Qwen 等兼容端点通过 `reasoning_content` 字段返回思考链，适配器会将其以
`Reasoning` 事件透出（流式与非流式一致）。由于 Chat Completions 的输入消息不接受
思考链字段（部分端点收到会直接报错），历史中的推理块不会回传。

## 档案校验与警告

请求构建时会对照模型档案（内置目录或 `with_profile` 注入）校验通用参数，档案未知时跳过：

- 模型不支持推理却设置了档位：丢弃并给出 `Unsupported` 警告；
- 档案未声明可关闭推理却要求 `ReasoningEffort::None`：不发送关闭指令并给出 `Compatibility` 警告；
- 档位不在模型支持列表里：有降级别名时降级并给出 `Compatibility` 警告，否则发送前报 `UnsupportedFeature`；
- 档案声明不接受 `temperature`：丢弃并给出 `Unsupported` 警告。

警告随流式的 `StreamStart.warnings` 与非流式的 `GenerateResult.warnings` 返回，
Agent 层会以 `model.warnings` 事件呈现。

## HTTP 配置

`with_client` 可传入自定义 `reqwest::Client`；默认客户端设置 30 秒连接超时与 300 秒读取超时
（相邻两次收到字节的间隔），不设整体超时。

## 厂商扩展

`CallOptions.provider_options` 的 `openai` 对象原样并入请求正文：Chat Completions 是众多
兼容端点的通用协议，各家私有字段（如 Qwen 的 `enable_thinking`、vLLM 的 `chat_template_kwargs`、
只认 `max_tokens` 的旧端点）无法穷举，因此采用透传。但已由通用参数写入的字段
（`model`、`messages`、`stream`、`temperature`、`tools` 等）不允许覆盖，会在发送前以
`UnsupportedFeature` 拒绝，避免 `CallOptions` 上的设置被悄悄改掉。

联调工具通过位置参数 `chat`（或 `--protocol openai-chat`）选择该协议，根地址仍然不包含
`/chat/completions` 后缀。

```powershell
cargo xtask probe chat            # 全套：generate、stream 用 mixed.json，agent 用 chat-tools.json
cargo xtask probe chat agent      # 只跑 Agent 场景
```

`chat-tools.json` 与 `responses-tools.json` 是同一任务：附件、函数调用、工具结果、两轮历史
回放和最终 JSON 的断言完全一致；由于 Chat Completions 不提供可见推理摘要、推理事件和
推理历史回放，该场景不包含这三类断言，也不携带 Responses 专属的 `reasoning.summary` 选项。
