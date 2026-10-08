# 真实 API 联调

在当前仓库运行 `cargo xtask probe`，通过 `--protocol` 选择协议，通过 `--mode`
选择联调流程。当前已接入 `openai-responses`。Chat Completions、Anthropic 等协议
待适配器实现后接入同一入口。

## 快速开始

在仓库根目录的 `.env.local` 中填写配置（此文件已被 Git 忽略）：

```dotenv
PROBE_API_KEY='你的 API Key'
PROBE_MODEL='你有权限使用的模型 ID'
PROBE_BASE_URL='https://api.openai.com/v1'
```

新检出仓库时可将 `.env.local.example` 复制为 `.env.local`。然后在 PowerShell 直接运行：

```powershell
cargo xtask probe --protocol openai-responses --mode generate
cargo xtask probe --protocol openai-responses --mode stream
cargo xtask probe --protocol openai-responses --mode agent
```

每条命令会执行独立的真实联调，可能产生 API 费用。端点必须支持所选协议。
Responses 的根地址包含版本路径，但不要包含 `/responses`，适配器会自动追加。

帮助命令不读取凭据、不发起请求：

```powershell
cargo xtask probe --help
```

## 配置与优先级

| 配置 | 优先级或默认值 |
| --- | --- |
| 协议 | `--protocol`，默认 `openai-responses` |
| 运行方式 | `--mode`，默认 `stream` |
| 模型 ID | `--model` → `PROBE_MODEL` → 对应协议的环境变量；无内置默认模型 |
| 根地址 | `--base-url` → `PROBE_BASE_URL` → 对应协议的环境变量 → 协议默认地址 |
| API Key | 指定 `--api-key-env` 时只读指定变量；否则 `PROBE_API_KEY` → 对应协议的环境变量 |
| 提示 | `--prompt`；默认普通回复或加法工具联调提示 |
| 整次联调超时 | `--timeout-secs`，默认 120 秒，可设 1～3600 |
| 每次输出 token 上限 | `--max-output-tokens`，默认 4096 |
| Agent 调用次数上限 | `--max-steps`，默认 4，可设 2～100，仅适用于 agent |
| 原始流事件 | `--raw`，额外透出协议原始 JSON |

Responses 兼容已有的 `OPENAI_API_KEY`、`OPENAI_MODEL`、`OPENAI_BASE_URL`。
显式指定的空配置会报错，不会悄悄切换到另一组凭据。

`.env.local` 作为变量的本地来源：对同名变量，进程环境覆盖文件值；
之后按上表选择 `PROBE_*` 或协议对应的别名，命令行选项优先级最高。
例如进程中的 `PROBE_MODEL` 覆盖文件中的 `PROBE_MODEL`；
文件中的 `PROBE_MODEL` 仍优先于协议别名 `OPENAI_MODEL`。

文件位置固定为仓库根目录，从子目录启动也使用同一份配置。文件缺失时继续使用
进程环境；文件不可读或格式错误时明确报错，不回显可能含有密钥的原始行。
支持 dotenv 注释、引号、`export` 和变量引用，重复声明以最后一次为准。
密钥含 `$` 时可用单引号保留字面值。UTF-8 BOM 与 Windows 换行均可解析。

该文件只用于联调配置，不修改进程环境，也不会由 `cargo xtask check` 自动加载。
本命令不搜索其他 `.env` 文件，不通过命令行参数接收密钥本身。

例如，同一协议切换到另一组端点配置：

```powershell
$env:TEAM_API_KEY = "另一组 API Key"
cargo xtask probe --protocol openai-responses --mode stream --model "模型 ID" --base-url "https://your-endpoint.example/v1" --api-key-env TEAM_API_KEY --prompt "用一句话介绍 Rust"
```

## 三种模式

**generate**：调用 `LanguageModel::do_generate`，输出结构化结果、结束原因、
usage、响应元数据，以及适配器提供的请求体和响应头。

**stream**：调用 `LanguageModel::do_stream`，逐个输出 `StreamPart`，
再通过 `StreamAccumulator` 汇总。流内错误、传输失败、未完成、输出截断或没有文本
都会让联调以非零状态退出。传输失败时尽可能输出已经积累的部分结果。

**agent**：调用项目的 `run_agent_loop`，注册一个只做整数加法的本地 `add` 工具。
第一轮指定调用 `add`；下一轮禁用工具，要求根据结果回答。联调同时验证：

- 至少完成两次模型响应；
- `add` 实际执行成功；
- 工具结果进入下一轮模型请求；
- 最后得到正常结束的文本回复。

仅收到文本回复而没有完成工具调用链路时，Agent 联调会失败。
联调关闭 Agent 自动重试，并限制模型调用次数；超时和 Ctrl+C 会丢弃当前流程、
释放本地网络等待。结束本地等待不保证服务端已经停止计费。

## 诊断输出

标准输出每行一个 JSON 对象，结构为 `{"type": "...", "data": ...}`：

| type | 内容 |
| --- | --- |
| `probe_start` | 协议、模式、模型、根地址和超时 |
| `model_request` | 每次调用的实际通用 `CallOptions`，包括历史和工具声明 |
| `stream_part` | 核心模型事件，开启 `--raw` 后也包含原始 SSE JSON |
| `model_result` | 普通生成或流聚合后的结果、usage 和结束原因 |
| `model_error` | 模型错误、底层原因和可用的厂商错误载荷 |
| `partial_result` | 传输失败时已经积累的结果 |
| `agent_event` | Agent 运行生命周期及工具事件 |
| `agent_result` | Agent 历史、轮数、总用量和结束原因 |
| `probe_finish` | 正常收尾或超时后的联调成功标记及耗时 |
| `probe_error` | 失败描述；命令返回非零退出码 |

参数校验错误直接输出到标准错误。Ctrl+C 会输出 `probe_error`，不等待正常的
`probe_finish`。API Key、认证头、Cookie 和加密推理字段会脱敏，其他请求内容用于诊断。

`model_request` 是适配器收到的通用入参；厂商实际 HTTP 请求体仅在
`GenerateResult.request_body` 提供时可见，不把通用入参误标为 HTTP 线上的原始请求。
Agent 模式同时展示模型事件和映射后的 Agent 事件，方便比较两层行为。

例如观察原始流并保存诊断：

```powershell
cargo xtask probe --protocol openai-responses --mode stream --raw --timeout-secs 180
cargo xtask probe --protocol openai-responses --mode agent > probe.jsonl
```

普通和流式联调要求结束原因是 `Stop` 且文本非空；`Length` 表示验证尚未完成，
可检查推理用量并按需调整输出上限。服务端拒绝文本仍可能是正常 `Stop`，
命令验证协议运行是否完成，回复语义需要查看实际内容。

## 调试与扩展

IDE 的调试目标选择 `xtask` 二进制，程序参数使用
`probe --protocol openai-responses --mode stream`，在启动环境中配置模型和凭据。
可在 `xtask/src/probe/observe.rs` 观察每轮调用，进入相应适配器的请求或解析模块打断点。

新增协议需要实现 `LanguageModel`，然后在 `probe/config.rs` 登记协议名称和默认配置，
在 `probe/factory.rs` 增加模型创建分支。`runner.rs`、Agent 场景和诊断输出继续复用。
厂商与协议分开：接入使用现有协议的新端点只需换配置。

自动化门禁仍运行 `cargo xtask check`，不会执行真实联调。
联调工具的测试使用注入配置、内存输出、脚本模型与虚拟时钟，无需密钥或网络：

```powershell
cargo test -p xtask probe::
```
