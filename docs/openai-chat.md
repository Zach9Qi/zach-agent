# OpenAI Chat Completions API

`zach-ai` 提供 `OpenAiChatCompletionsModel`（别名 `OpenAiChatModel`），实现
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

PDF 输入需要使用已上传的 OpenAI `file_id` 或内联 Base64 数据；Chat Completions 不接受
PDF URL。服务端托管工具、音视频和助手历史中的原始推理块暂不在此适配器中回放。

联调工具可以通过 `--protocol openai-chat` 选择该协议，根地址仍然不包含
`/chat/completions` 后缀。

`scenarios/probe/mixed-tools.json` 可同时用于 Responses 和 Chat Completions 联调。场景中的
推理摘要、推理事件和推理历史回放断言通过 `requires` 声明能力：Responses 适配器声明支持
时会严格验证，Chat Completions 未声明这些官方协议之外的正文能力时会明确跳过；附件、函数
调用、工具结果、两轮历史回放和最终 JSON 仍然严格验证。
