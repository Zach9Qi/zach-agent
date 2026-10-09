# 真实 API 场景联调

`cargo xtask probe --mode <模式> --scenario <场景.json>` 加载完整输入和预期，通过统一的
`LanguageModel` / `run_agent_loop` 访问真实端点。当前支持 `openai-responses`、`openai-chat`
和 `anthropic-messages`。
新增输入组合通常只需写 JSON；新增断言或可执行工具时才需扩展联调代码。

## 快速开始

复制仓库根目录 `.env.local.example` 为 `.env.local`，填写连接信息：

```dotenv
PROBE_API_KEY='你的 API Key'
PROBE_MODEL='支持所需能力的模型 ID'
PROBE_BASE_URL='https://api.openai.com/v1'
```

在仓库根目录运行：

```powershell
cargo xtask probe --help
cargo xtask probe --mode generate --scenario scenarios/probe/mixed.json
cargo xtask probe --mode stream --scenario scenarios/probe/mixed.json
cargo xtask probe --mode agent --scenario scenarios/probe/responses-tools.json
```

每条命令执行一个真实场景，可能产生 API 费用。模型及端点必须支持该场景使用的
图片、PDF、推理或工具能力。帮助命令不读取凭据、不加载附件，也不访问网络。
自动化门禁只跑离线测试，不执行这些真实请求。

`--mode` 可选 `generate`、`stream`、`agent`，省略时为 `stream`。场景文件只定义输入、
预期和执行所需配置，同一个文件可切换执行方式，无需复制 JSON。普通场景也能使用
`--mode agent`；未配置 `agent` 对象时不注册工具，最多调用模型 4 次。

## 场景结构与标准格式

| 字段 | 含义 |
| --- | --- |
| `request` | 必填：沿用 `zach_ai_core::CallOptions` 的 JSON 格式，文件块支持本地 `path` 简写 |
| `expect` | 必填：非空断言数组，全部通过才成功 |
| `schema_file` | 可选：共享输出 Schema 的文件路径，加载到标准 `request.response_format.schema` |
| `timeout_secs` | 整次运行超时，默认 120，范围 1～3600 秒 |
| `agent` | 可选，仅用于 `--mode agent`：执行工具、轮数限制及后续工具策略 |

`request.prompt.messages` 使用标准 `Message`，用户内容使用标准 `UserPart`。
图片和 PDF 都是 `type: "file"`，由 `media_type` 区分。文本、文件块按数组顺序
保留，支持多条消息、系统提示以及初始助手和工具历史。

以下是文本与本地图片组合的完整示例；将它保存在 `scenarios/probe/` 下即可使用已有附件。

```json
{
  "request": {
    "prompt": {
      "messages": [
        {
          "role": "user",
          "content": [
            { "type": "text", "text": "读取图片中的数字，只返回一个数字。" },
            {
              "type": "file",
              "media_type": "image/png",
              "path": "fixtures/image-a.png"
            }
          ]
        }
      ]
    },
    "max_output_tokens": 4096
  },
  "expect": [
    { "type": "finish_reason", "value": "stop" },
    { "type": "text_equals", "value": "137" }
  ]
}
```

`request` 沿用已有标准类型；文件块中的 `path` 是场景加载器提供的简写，加载时移除
`path` 并补齐标准 `data`，最终构造完整的 `CallOptions`。
`reasoning: "high"` 是通用推理强度；OpenAI 的摘要配置放在
`request.provider_options.openai.reasoning.summary: "auto"`。
原始协议事件使用 `request.include_raw_chunks: true`。
输出上限由 `request.max_output_tokens` 决定；省略时沿用库和端点默认值，内置场景使用 8192。

### 消息中的本地附件

本地附件直接在对应文件块中填写 `path`，例如：

```json
{
  "type": "file",
  "media_type": "application/pdf",
  "filename": "report-a.pdf",
  "path": "fixtures/report-a.pdf"
}
```

路径相对于**场景文件所在目录**解析，也支持绝对路径。文字、图片和 PDF 可直接按
需要混排；移动消息或文件块时无需维护额外下标。同一个文件也可以在不同位置引用。
加载器读取附件后填入标准 `FileData::Data`，保留块顺序、MIME、文件名和厂商配置。

同一文件的 `path` 与标准 `data` 必须二选一；同时提供时会报错，即使 `data` 是空值
或空数据占位符。两者都没有时，也会在请求发出前报错。`path` 必须是非空字符串，
本地文件块必须提供非空 `media_type`；文件不可读或本地文件为空都会失败。
助手历史中的 `file`、`reasoning_file` 以及助手或工具消息的复合工具结果中的
`file` 同样支持 `path`。工具参数、JSON 结果、自定义块及厂商选项中的业务数据不会被
当作附件处理。旧的顶层 `assets` 已移除，继续使用会报未知字段错误。

已经使用标准内联 Base64、URL、厂商文件 ID 或内联文本的请求继续填写 `data`，
无需 `path`。对应的 `FileData` 分别为：

- `{"type":"data","data":"AQID"}`（仅说明 Base64 结构，不是有效图片）。
- `{"type":"url","url":"https://example.com/report.pdf"}`。
- `{"type":"reference","reference":{"openai":"file-abc"}}`。
- `{"type":"text","text":"内联文档内容"}`。

联调工具不会替你上传文件创建厂商 ID，也不把本地路径发送给远端。
当前 Responses 适配器支持图片、PDF 与内联文本，其他格式以适配器能力为准。

### 从 Rust 类型生成并引用输出 Schema

输出结构在 `xtask/src/probe_schemas/types.rs` 中定义，使用
`#[derive(Serialize, Deserialize, JsonSchema)]`，通过 `#[serde(deny_unknown_fields)]`
关闭额外字段。运行以下命令即可生成 `scenarios/probe/schemas/` 下的共享文件：

```powershell
cargo xtask probe-schemas
```

该命令无需密钥或网络，从任何工作目录启动都写入仓库固定目录。
新结构在 `probe_schemas.rs` 的 `schemas()` 中登记一次；修改已有结构后重新生成即可。
生成文件随仓库提供，日常运行场景不需要先生成。离线测试会比较 Rust 类型与生成文件，
防止改了类型却忘记更新 Schema。

场景只需添加一个字段：

```json
"schema_file": "schemas/attachment-summary.json"
```

路径相对于**场景文件所在目录**解析，也支持绝对路径。省略
`request.response_format` 时加载器自动选择 JSON；如果需要自定义名称或说明，仍使用
标准格式的 `name` / `description`，加载器只补齐 `schema`：

```json
"response_format": {
  "type": "json",
  "name": "attachment_summary",
  "description": "附件读取与汇总结果"
}
```

`schema_file` 与内联 `request.response_format.schema` 二选一，不能同时提供，
也不能与文本输出格式混用。文件不可读、不是 UTF-8、不是有效 JSON 或根节点不是对象，
都会在模型调用前报错。文件内容作为完整 Schema 原样装入标准请求；支持文件内部的
`$defs` / `$ref`，加载器不递归读取外部引用，具体 Schema 关键字由所选协议和模型校验。

Responses 适配器将带 Schema 的标准格式映射为 `text.format.type: json_schema`、
`strict: true`。只有 `{"type":"json"}` 且没有 Schema 时，才使用普通 `json_object` 模式。
内置混合结果类型将所有字段设为必填，并禁止额外字段；Schema 约束输出结构，
`expect` 继续检查实际答案，预期数值不会写入生成文件或发送给模型。

两份内置场景共用 `AttachmentSummary` 生成的 `schemas/attachment-summary.json`。
普通 JSON 模式的映射由离线测试覆盖，内置真实场景统一验证 Schema 约束。

## 预期与成功判定

所有模式都拒绝传输失败、流内错误、缺少完整响应、超时和中止。
内容、结束原因和轮数由 `expect` 显式决定。普通成功场景应包含
`finish_reason: stop`，这样输出截断（`length`）不能被当成成功。
也可配置 `tool_calls` 来验证只产生工具调用的单次模型请求。

| `type` | 参数与含义 |
| --- | --- |
| `finish_reason` | `value`：最后一轮统一结束原因，如 `stop`、`tool_calls` |
| `text_nonempty` | 最后一轮回答去掉首尾空白后非空 |
| `text_contains` | `value`：最后一轮回答包含非空字符串 |
| `text_equals` | `value`：回答去掉首尾空白后与给定文本相等 |
| `json_equals` | `pointer`、`value`：将最后一轮回答整体解析为 JSON，指定节点精确相等 |
| `json_type` | `pointer`、`kind`：节点类型为 `object/array/string/number/boolean/null` |
| `reasoning` | `evidence`：`any/summary/tokens/metadata`，检查本次任一轮实际响应 |
| `event` | `event`、`min`：统一 `StreamPart` 事件名称及最少次数，仅用于流式或 Agent |
| `tool_call` | `name`、可选 `input`：本次模型实际调用指定工具，指定 input 时精确匹配 JSON |
| `tool_result` | `name`、`value`：本次 Agent 新增的成功 JSON 工具结果精确匹配 |
| `steps` | `min`、`max`：模型完成的轮数范围，包含边界 |
| `replay` | `content`：`input/reasoning/tool_results`，检查 Agent 实际后续请求 |

断言没有条件执行：写进 `expect` 的每一条都会评估并计入成败。协议不提供某类内容
（例如 Chat Completions 没有可见推理摘要）时，不要在断言上加条件，而是为该协议单独维护
一份场景文件、去掉对应断言（见内置案例）。旧的 `requires` 字段已删除，继续使用会报未知字段错误。

`json_equals` 区分缺失字段、`null`、字符串和数字；根节点的 Pointer 使用空字符串。
JSON 回答不能带 Markdown 围栏；可用 `request.response_format` 请求 JSON 输出。
`json_type` 只检查节点类型，不是完整 JSON Schema 验证器。

推理证据彼此独立：`summary` 要求非空可见摘要；`tokens` 要求推理用量大于零；
`metadata` 要求推理内容块携带非空厂商元数据；`any` 满足其中任一项即可。
模型可能没有可见摘要，但仍返回推理用量或加密推理项。
`event` 使用 `reasoning_delta` 等统一名称，不使用厂商 SSE 名称。

执行方式由命令行决定。`generate` 模式使用 `event`，
或非 `agent` 模式使用 `tool_result/replay`，都会在读取附件和调用模型前报错。

`replay` 至少需要两次实际调用，且不能在没有对应内容时空判成功：

- `input`：所有后续请求保留首轮完整消息前缀，包括附件字节、顺序及元数据。
- `reasoning`：每轮输出的推理块在紧接着的请求中保留文本及完整厂商元数据。
- `tool_results`：本地工具调用及对应结果以相同调用 ID 出现在紧接着的请求中。

这些回传断言检查 `LanguageModel` 接收的通用请求；线上序列化仍由适配器负责。
预置在初始历史中的工具结果不能满足“本次执行成功”的断言。

## Agent 场景

`agent` 配置示例：

```json
{
  "tools": ["add"],
  "max_steps": 4,
  "next_tool_choice": { "type": "none" }
}
```

当前可执行工具只有纯整数加法 `add`，省略 `agent` 对象或使用 `tools: []` 均可验证
不执行工具的 Agent。配置了 `agent` 对象的场景必须使用 `--mode agent`，不会在
其他模式下静默忽略工具注册或后续轮次设置。
`max_steps` 是模型调用次数上限，默认 4，范围 1～100；自动重试关闭。
首轮策略由 `request.tool_choice` 指定，例如 `{"type":"tool","tool_name":"add"}`。
后续轮次仅在配置 `next_tool_choice` 时切换策略，省略则保留原策略。

Agent 循环根据可执行工具生成标准声明，因此该模式不允许同时设置 `request.tools`；
`generate/stream` 可直接使用 `request.tools` 验证模型的工具调用输出。
场景不会隐式插入提示词、强制加法任务或要求工具必须执行；这些行为都由输入与断言声明。
超时和 Ctrl+C 停止本地等待，不保证服务端已停止计费。

## 内置案例

| 文件（均位于 `scenarios/probe/`） | 可用 `--mode` | 覆盖内容 |
| --- | --- | --- |
| `mixed.json` | 全部 | 文本＋两张图片＋两份 PDF，共享 Schema、逐项校验及求和 |
| `responses-tools.json` | `agent` | `openai-responses`：共享 Schema＋混合附件＋推理摘要、事件与回放＋工具执行＋输入、工具结果回传 |
| `chat-tools.json` | `agent` | `openai-chat`：同上，但不带 Responses 专属的摘要选项，也不断言推理摘要、事件与回放 |
| `anthropic-tools.json` | `agent` | `anthropic-messages`：同 `responses-tools.json`，改用 `tool_choice: auto` 且不带 Schema |

日常真实联调只维护这几份混合场景；工具场景按协议各一份，文件名即 `--protocol`。
需要定位某类输入的问题时，可以临时删减混合
场景中的内容块和对应断言，无需长期维护文本、单图、单文件和单工具的重复案例。
配置冲突、错误响应、超时等边界继续由确定性的离线测试覆盖。

两张图片分别给出 137、263，两份 PDF 分别给出 421、89，总和 910。
数值只出现在附件和本地断言中，`expect` 不发送给模型。
混合场景还要求回显正文中的校验码，证明正文与各附件共同参与回答。

三份工具场景是同一任务的协议变体：提示词、附件、JSON 预期、工具调用参数与输入、
工具结果回传断言完全一致，只在协议相关的请求选项和推理断言上有差异。场景文件所见即所得，
不做跨文件合并；离线测试会比较这些共同部分，改动一份而漏改其他两份时会直接失败。
Chat Completions 不提供可见推理摘要、推理事件和推理回放，`chat-tools.json` 因此不包含
对应断言；输入和工具结果仍需在下一轮请求中完整回传。

`anthropic-tools.json` 专用于 `--protocol anthropic-messages`：Anthropic 在强制 `tool_choice`
的那一轮不会思考，且 JSON Schema 输出与工具调用叠加时模型常常跳过工具，因此该变体改用
`tool_choice: auto` 并去掉 Schema（Schema 输出已由 `mixed.json` 在 Anthropic 上覆盖），
使第一轮能同时产生思考块与工具调用，从而验证签名回放。

测试附件已随仓库提供，无需 Python 即可联调。
若需重新生成，可安装 Pillow 和 reportlab，执行
`python scenarios/probe/fixtures/generate.py --font <中文字体路径>`。

## 连接配置与迁移

命令行支持 `--scenario`、`--mode`、`--protocol`、`--model`、`--base-url`、
`--api-key-env` 和帮助。JSON 中的旧 `mode` 字段已删除，继续使用会报错；
执行方式只能由 `--mode` 指定。`--prompt/--raw/--timeout-secs/--max-steps/--max-output-tokens`
已移入场景对应字段，继续传入会报未知参数。

| 配置 | 优先级 |
| --- | --- |
| 协议 | `--protocol`，可选 `openai-responses`、`openai-chat`、`anthropic-messages`，默认 `openai-responses` |
| 执行模式 | `--mode`，默认 `stream` |
| 模型 | `--model` → `PROBE_MODEL`，无内置默认值 |
| 根地址 | `--base-url` → `PROBE_BASE_URL` → 协议默认值（OpenAI `https://api.openai.com/v1`，Anthropic `https://api.anthropic.com`） |
| 密钥 | 指定 `--api-key-env` 时只读指定变量；否则只读 `PROBE_API_KEY` |

已移除 `OPENAI_MODEL`、`OPENAI_BASE_URL` 和 `OPENAI_API_KEY` 的自动回退。
已有本地配置请改用 `PROBE_*`；`--api-key-env` 仍可显式选择任意密钥变量。

OpenAI 根地址包含版本路径，Anthropic 根地址不含 `/v1`；两者都不能包含 `/responses`、
`/chat/completions`、`/messages` 等端点后缀、认证信息、查询或片段。
显式空配置会报错，不会切换到另一组凭据。密钥不通过命令行参数或场景文件配置。
`.env.local` 固定从仓库根目录读取；同名进程变量覆盖文件值，随后按上表选择变量。
文件支持 dotenv 引号、注释、变量引用、重复声明（最后一次为准）、UTF-8 BOM 和 CRLF，
不会修改进程环境。缺失文件时沿用进程环境，不搜索其他 `.env` 文件。

## 诊断与离线检查

标准输出为逐行 JSON，结构为 `{"type":"...","data":...}`：

| type | 内容 |
| --- | --- |
| `probe_start` | 场景路径、协议、模式、模型、根地址和超时 |
| `model_request` | 每轮实际通用 `CallOptions`，包含历史和工具声明 |
| `stream_part` | 统一事件；开启 include_raw_chunks 后包含原始协议事件 |
| `model_result` | 每轮普通或流聚合后的结果、用量与结束原因 |
| `model_error` / `partial_result` | 模型错误或传输失败前的部分结果 |
| `agent_event` / `agent_result` | Agent 生命周期、执行事件及最终历史 |
| `assertion_result` | 断言索引（从 0 开始）、具体预期与是否通过 |
| `probe_finish` / `probe_error` | 运行是否成功、耗时或失败说明 |

任一断言失败返回非零退出码，并输出所有已评估断言，便于一次定位多个不匹配。
连接和 JSON 配置校验失败时不会开始请求；Ctrl+C 输出错误，不等待正常收尾。
API Key、认证头、Cookie 和加密推理字段脱敏；其他请求内容（含附件数据）用于诊断。
线上 HTTP 请求体仅在适配器提供 `GenerateResult.request_body` 时可见。

```powershell
cargo xtask probe --mode agent --scenario scenarios/probe/responses-tools.json > target/probe.jsonl
cargo test -p xtask probe::
cargo run -p xtask -- check
```

离线测试使用注入配置、内存附件加载器、脚本模型和虚拟时钟，不使用凭据或网络。
新增协议只需实现 `LanguageModel`，在 `config.rs` 和 `factory.rs` 注册；
场景、执行流程和通用断言继续复用。
