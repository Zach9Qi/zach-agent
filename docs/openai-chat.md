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
推理强度、非流式生成与 SSE 流式生成。`max_output_tokens` 在 `openai` 身份下发送为
`max_completion_tokens`（o 系列起官方端点只认此字段），其他厂商身份下发送为 `max_tokens`。
函数工具与 JSON Schema 输出的严格模式都由调用方显式选择（`with_strict`），未指定时沿用服务端
默认的不严格校验，普通 Schema 不会因严格模式的额外约束被拒绝。用户消息只有一个文本块时以
字符串发送，多块（含多段文本）以部件数组发送以保留边界。流式请求自动设置 `stream_options.include_usage`，
以便在末尾返回完整用量。工具调用参数会按增量事件拼接，并在结束时发出完整调用；
`id` 晚于参数到达时参数先缓存，始终没有 `id` 的调用无法与结果配对，会以 `Error` 事件报告
而不是静默丢弃。

接入 Ollama、vLLM、LM Studio 等无需鉴权的兼容端点时 API Key 传空字符串即可，
此时不发送 `Authorization` 头。

接入 DeepSeek、Qwen 等第三方兼容端点时用 `with_provider("deepseek")` 声明厂商身份：
`provider()` 与错误中的厂商名随之变化，模型档案改按该厂商在内置目录中查询
（`("deepseek", "deepseek-v4-pro")` 这类条目无需再手动注入），`provider_options` 也改读该厂商键
（未提供时回退 `openai` 键）。响应元数据与回放标记（如 `refusal`）属于协议层，固定使用 `openai` 键。
内置目录未收录的模型仍可用 `with_profile(ModelProfile)` 注入档案。

PDF 输入需要使用已上传的 OpenAI `file_id` 或内联 Base64 数据；Chat Completions 不接受
PDF URL。服务端托管工具、音视频和助手历史中的原始推理块暂不在此适配器中回放。

DeepSeek、Qwen、xAI 等端点通过 `reasoning_content` 字段、OpenRouter、Ollama、Groq 等通过
`reasoning` 字段返回思考链，适配器都会将其以 `Reasoning` 事件透出（流式与非流式一致）。思考阶段伴发的空 `content` 占位不会提前
结束推理块；推理块在首个非空正文、拒绝或工具调用增量到达时关闭，收尾后迟到的增量
一律忽略。由于 Chat Completions 的输入消息不接受
思考链字段（部分端点收到会直接报错），历史中的推理块不会回传。

## 档案校验与警告

请求构建时会对照模型档案（内置目录或 `with_profile` 注入）校验通用参数，档案未知时跳过：

- 模型不支持推理却设置了档位：丢弃并给出 `Unsupported` 警告；
- 档案未声明可关闭推理却要求 `ReasoningEffort::None`：不发送关闭指令并给出 `Compatibility` 警告；
- 档位不在模型支持列表里：有降级别名时降级并给出 `Compatibility` 警告，否则发送前报 `UnsupportedFeature`；
  本适配器的别名为 `Max` → `Xhigh`（`max` 仅 gpt-5.6 及更新代际的档案声明，档案未知时原样发送）；
- 档案声明不接受 `temperature`：丢弃并给出 `Unsupported` 警告；
- 声明了非 `openai` 的厂商身份且要求 `ReasoningEffort::None`：不发送 `reasoning_effort`（`none`
  只是 OpenAI 的约定）并给出 `Compatibility` 警告，请改用 `provider_options` 传入该端点的关闭字段
  （如 Qwen 的 `enable_thinking: false`、DeepSeek 的 `thinking: {"type": "disabled"}`）。

警告随流式的 `StreamStart.warnings` 与非流式的 `GenerateResult.warnings` 返回，
Agent 层会以 `model.warnings` 事件呈现。

## 流的收尾

`finish_reason` 到达后等用量块（或 `[DONE]`）再发出 `Finish`；不支持 `include_usage` 又省掉
`[DONE]` 直接断开的端点，在 `finish_reason` 已到时同样按完成收尾（用量为空）。`finish_reason`
之前连接断开视为传输错误。流内 `error` 分块使本轮结果为 `Error`，其后的断开不再额外报告为
传输错误。

## HTTP 配置

默认客户端只设 30 秒连接超时；读取与整体超时按请求类型区分：

- 非流式请求（`do_generate`）在服务端生成完之前收不到任何字节，默认整体超时 10 分钟，
  `with_generate_timeout(Some(..))` 可放宽，`None` 表示不限制；
- 流式请求（`do_stream`）等待响应头、以及相邻两次收到数据之间默认最多 5 分钟，
  `with_stream_idle_timeout` 可调整；超时以可重试的 `StreamError` 结束流。

`with_client` 可传入自定义 `reqwest::Client` 配置连接池与代理；客户端级的 `timeout` /
`read_timeout` 会与上述规则叠加，注意 `read_timeout` 同样会约束非流式请求等待响应头的时间。

HTTP 401/403 映射为 `Authentication`，429 映射为 `RateLimit`，408 与 5xx 映射为可重试的
`ServerError`，其他非成功状态映射为 `ProviderError`；传输失败映射为 `StreamError`。

## 厂商扩展

`CallOptions.provider_options` 中声明厂商对应的对象（默认 `openai`，`with_provider` 声明后优先读
该厂商键、缺省回退 `openai`）原样并入请求正文：Chat Completions 是众多
兼容端点的通用协议，各家私有字段（如 Qwen 的 `enable_thinking`、vLLM 的 `chat_template_kwargs`）
无法穷举，因此采用透传。但已由通用参数写入的字段
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
