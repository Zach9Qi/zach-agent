# LanguageModelV4 格式

源码：`packages/provider/src/language-model/v4/`

```ts
import type { JSONSchema7 } from 'json-schema';

/** JSON 可序列化值 */
type JSONValue =
  | null
  | string
  | number
  | boolean
  | JSONObject
  | JSONValue[];

type JSONObject = { [key: string]: JSONValue | undefined };

/**
 * 厂商专有入参。外层 key 是厂商名，内层是该厂商自己的字段。
 * 例：{ anthropic: { cacheControl: { type: 'ephemeral' } } }
 */
type SharedV4ProviderOptions = Record<string, JSONObject>;

/**
 * 厂商专有出参。结构和 ProviderOptions 一样，但是响应侧元数据。
 */
type SharedV4ProviderMetadata = Record<string, JSONObject>;

/**
 * 厂商侧已有文件的引用，无需重新上传。
 * 例：{ openai: 'file-abc123', anthropic: 'file-xyz789' }
 * 不能带 type 字段，以免和下面的 tagged union 冲突。
 */
type SharedV4ProviderReference = Record<string, string> & { type?: never };

/** 文件本体，用 type 区分怎么携带 */
type SharedV4FileData =
  | { type: 'data'; data: Uint8Array | string } // 原始字节或 base64
  | { type: 'url'; url: URL } // 指向文件的 URL
  | { type: 'reference'; reference: SharedV4ProviderReference } // 厂商已有文件 ID
  | { type: 'text'; text: string }; // 内联文本文档

/** HTTP 响应头 */
type SharedV4Headers = Record<string, string>;

/** 调用警告：不支持、兼容降级、弃用、其他 */
type SharedV4Warning =
  | { type: 'unsupported'; feature: string; details?: string }
  | { type: 'compatibility'; feature: string; details?: string }
  | { type: 'deprecated'; setting: string; message: string }
  | { type: 'other'; message: string };

// ---------------------------------------------------------------------------
// LanguageModelV4
// ---------------------------------------------------------------------------

/** 语言模型规范 v4。各厂商实现这个接口。 */
type LanguageModelV4 = {
  /** 规范版本，固定为 'v4' */
  readonly specificationVersion: 'v4';
  /** 厂商 ID，如 'openai' */
  readonly provider: string;
  /** 厂商侧模型 ID */
  readonly modelId: string;
  /**
   * 该模型原生支持、无需 SDK 下载的 URL。
   * key 是媒体类型（如 audio/*、application/pdf），value 是匹配 URL 的正则。
   */
  supportedUrls:
    | PromiseLike<Record<string, RegExp[]>>
    | Record<string, RegExp[]>;
  /** 非流式生成 */
  doGenerate(options: LanguageModelV4CallOptions): PromiseLike<LanguageModelV4GenerateResult>;
  /** 流式生成 */
  doStream(options: LanguageModelV4CallOptions): PromiseLike<LanguageModelV4StreamResult>;
};

// ---------------------------------------------------------------------------
// 调用入参（转成厂商格式之前）
// ---------------------------------------------------------------------------

type LanguageModelV4CallOptions = {
  /** 标准化消息列表，不是用户直接传入的 prompt */
  prompt: LanguageModelV4Prompt;
  /** 最多生成多少 token */
  maxOutputTokens?: number;
  /** 采样温度，范围由厂商和模型决定 */
  temperature?: number;
  /** 遇到任一停词序列就停止生成 */
  stopSequences?: string[];
  /** Nucleus sampling */
  topP?: number;
  /** 每步只从概率最高的 K 个 token 中采样 */
  topK?: number;
  /** 降低重复「已经在 prompt 里出现过的信息」的倾向 */
  presencePenalty?: number;
  /** 降低反复使用同一词/短语的倾向 */
  frequencyPenalty?: number;
  /** 输出格式，默认文本 */
  responseFormat?:
    | { type: 'text' }
    | {
        type: 'json';
        /** 期望输出符合的 JSON Schema */
        schema?: JSONSchema7;
        /** 输出名称，部分厂商用来给模型额外指引 */
        name?: string;
        /** 输出描述，部分厂商用来给模型额外指引 */
        description?: string;
      };
  /** 随机采样种子。模型支持时，相同调用应得到确定性结果 */
  seed?: number;
  /** 本次调用可用的工具 */
  tools?: Array<LanguageModelV4FunctionTool | LanguageModelV4ProviderTool>;
  /** 工具选择策略，默认 auto */
  toolChoice?: LanguageModelV4ToolChoice;
  /** 流式调用时是否把厂商原始 chunk 透出 */
  includeRawChunks?: boolean;
  /** 取消本次调用 */
  abortSignal?: AbortSignal;
  /** 额外 HTTP 头，仅对 HTTP 类厂商有效 */
  headers?: Record<string, string | undefined>;
  /**
   * 推理强度。
   * provider-default：用厂商默认；none：关闭推理；其余为从弱到强的档位。
   */
  reasoning?:
    | 'provider-default'
    | 'none'
    | 'minimal'
    | 'low'
    | 'medium'
    | 'high'
    | 'xhigh';
  /** 厂商专有入参，按厂商名透传 */
  providerOptions?: SharedV4ProviderOptions;
};

// ---------------------------------------------------------------------------
// Prompt / Message
// ---------------------------------------------------------------------------

/** 一条 prompt 就是一组消息 */
type LanguageModelV4Prompt = Array<LanguageModelV4Message>;

type LanguageModelV4Message = (
  | {
      role: 'system';
      /** system 只有纯文本，没有 part 数组 */
      content: string;
    }
  | {
      role: 'user';
      content: Array<LanguageModelV4TextPart | LanguageModelV4FilePart>;
    }
  | {
      role: 'assistant';
      content: Array<
        | LanguageModelV4TextPart
        | LanguageModelV4FilePart
        | LanguageModelV4CustomPart
        | LanguageModelV4ReasoningPart
        | LanguageModelV4ReasoningFilePart
        | LanguageModelV4ToolCallPart
        | LanguageModelV4ToolResultPart
      >;
    }
  | {
      role: 'tool';
      content: Array<
        | LanguageModelV4ToolResultPart
        | LanguageModelV4ToolApprovalResponsePart
      >;
    }
) & {
  /** 这条消息上的厂商专有选项 */
  providerOptions?: SharedV4ProviderOptions;
};

// ---------------------------------------------------------------------------
// Prompt 内容块
// ---------------------------------------------------------------------------

/** 普通文本 */
interface LanguageModelV4TextPart {
  type: 'text';
  text: string;
  providerOptions?: SharedV4ProviderOptions;
}

/** 附件：图片、PDF、音频、视频、文本文件等 */
interface LanguageModelV4FilePart {
  type: 'file';
  filename?: string;
  data: SharedV4FileData;
  /**
   * 媒体类型。可以是完整 IANA 类型（image/png），
   * 也可以是顶层段（image / audio / video / text）。
   * image/* 这类通配等价于顶层段 image。
   */
  mediaType: string;
  providerOptions?: SharedV4ProviderOptions;
}

/** 历史推理文本，用于把上一轮思考过程回放给模型 */
interface LanguageModelV4ReasoningPart {
  type: 'reasoning';
  text: string;
  providerOptions?: SharedV4ProviderOptions;
}

/** 推理过程中产生的文件。只支持 data / url，没有 reference / text */
interface LanguageModelV4ReasoningFilePart {
  type: 'reasoning-file';
  data:
    | { type: 'data'; data: Uint8Array | string }
    | { type: 'url'; url: URL };
  mediaType: string;
  providerOptions?: SharedV4ProviderOptions;
}

/**
 * 无法映射到标准 part 的厂商专有内容。
 * kind 格式：{provider}.{provider-type}
 */
interface LanguageModelV4CustomPart {
  type: 'custom';
  kind: `${string}.${string}`;
  providerOptions?: SharedV4ProviderOptions;
}

/** 模型发出的工具调用（通常来自上一轮 assistant 输出） */
interface LanguageModelV4ToolCallPart {
  type: 'tool-call';
  /** 用来和对应的 tool result 配对 */
  toolCallId: string;
  toolName: string;
  /** 调用参数，应是可 JSON 序列化、且符合该工具 input schema 的对象 */
  input: unknown;
  /** true 表示由厂商执行；未设或 false 表示由客户端执行 */
  providerExecuted?: boolean;
  providerOptions?: SharedV4ProviderOptions;
}

/** 某次工具调用的结果。可出现在 assistant 或 tool 消息里 */
interface LanguageModelV4ToolResultPart {
  type: 'tool-result';
  toolCallId: string;
  toolName: string;
  output: LanguageModelV4ToolResultOutput;
  providerOptions?: SharedV4ProviderOptions;
}

/** 用户对「厂商侧执行工具」的审批回复。只出现在 tool 消息里 */
interface LanguageModelV4ToolApprovalResponsePart {
  type: 'tool-approval-response';
  /** 对应厂商发出的 tool-approval-request 的 ID */
  approvalId: string;
  /** true 同意执行，false 拒绝 */
  approved: boolean;
  reason?: string;
  providerOptions?: SharedV4ProviderOptions;
}

/** 工具结果载荷 */
type LanguageModelV4ToolResultOutput =
  | {
      /** 直接发给厂商 API 的文本结果 */
      type: 'text';
      value: string;
      providerOptions?: SharedV4ProviderOptions;
    }
  | {
      type: 'json';
      value: JSONValue;
      providerOptions?: SharedV4ProviderOptions;
    }
  | {
      /** 用户拒绝执行该工具 */
      type: 'execution-denied';
      reason?: string;
      providerOptions?: SharedV4ProviderOptions;
    }
  | {
      type: 'error-text';
      value: string;
      providerOptions?: SharedV4ProviderOptions;
    }
  | {
      type: 'error-json';
      value: JSONValue;
      providerOptions?: SharedV4ProviderOptions;
    }
  | {
      /** 文本 + 文件 + 自定义块的混合结果 */
      type: 'content';
      value: Array<
        | {
            type: 'text';
            text: string;
            providerOptions?: SharedV4ProviderOptions;
          }
        | {
            type: 'file';
            data: SharedV4FileData;
            mediaType: string;
            filename?: string;
            providerOptions?: SharedV4ProviderOptions;
          }
        | {
            type: 'custom';
            providerOptions?: SharedV4ProviderOptions;
          }
      >;
    };

// ---------------------------------------------------------------------------
// 工具定义
// ---------------------------------------------------------------------------

/** 客户端定义、通常由客户端执行的函数工具 */
type LanguageModelV4FunctionTool = {
  type: 'function';
  /** 同一次调用内唯一 */
  name: string;
  /** 给模型看的用途说明 */
  description?: string;
  /** 输入参数的 JSON Schema */
  inputSchema: JSONSchema7;
  /** 可选输入示例 */
  inputExamples?: Array<{ input: JSONObject }>;
  /** 严格模式。支持的厂商会保证输入合法，但可能限制可用 schema */
  strict?: boolean;
  providerOptions?: SharedV4ProviderOptions;
};

/**
 * 某个厂商特有的工具。输入/输出 schema 由厂商定义，
 * 部分会在厂商侧执行（如 MCP）。
 */
type LanguageModelV4ProviderTool = {
  type: 'provider';
  /** 格式：<provider-id>.<unique-tool-name> */
  id: `${string}.${string}`;
  name: string;
  /** 配置参数，必须符合该厂商对该工具的约定 */
  args: Record<string, unknown>;
};

type LanguageModelV4ToolChoice =
  | { type: 'auto' } // 自动决定是否调用工具
  | { type: 'none' } // 禁止调用任何工具
  | { type: 'required' } // 必须调用至少一个可用工具
  | { type: 'tool'; toolName: string }; // 必须调用指定名称的工具

// ---------------------------------------------------------------------------
// 非流式结果
// ---------------------------------------------------------------------------

type LanguageModelV4GenerateResult = {
  /** 模型生成的有序内容 */
  content: Array<LanguageModelV4Content>;
  finishReason: LanguageModelV4FinishReason;
  usage: LanguageModelV4Usage;
  /** 厂商专有出参 */
  providerMetadata?: SharedV4ProviderMetadata;
  /** 发给厂商的请求体，用于遥测/调试 */
  request?: { body?: unknown };
  /** 厂商响应元数据，用于遥测/调试 */
  response?: LanguageModelV4ResponseMetadata & {
    headers?: SharedV4Headers;
    body?: unknown;
  };
  /** 如不支持的设置等警告 */
  warnings: Array<SharedV4Warning>;
};

/** 模型生成的内容块（输出侧） */
type LanguageModelV4Content =
  | LanguageModelV4Text
  | LanguageModelV4Reasoning
  | LanguageModelV4CustomContent
  | LanguageModelV4ReasoningFile
  | LanguageModelV4File
  | LanguageModelV4ToolApprovalRequest
  | LanguageModelV4Source
  | LanguageModelV4ToolCall
  | LanguageModelV4ToolResult;

type LanguageModelV4Text = {
  type: 'text';
  text: string;
  providerMetadata?: SharedV4ProviderMetadata;
};

type LanguageModelV4Reasoning = {
  type: 'reasoning';
  text: string;
  providerMetadata?: SharedV4ProviderMetadata;
};

/** kind 格式：{provider}.{provider-type} */
type LanguageModelV4CustomContent = {
  type: 'custom';
  kind: `${string}.${string}`;
  providerMetadata?: SharedV4ProviderMetadata;
};

/** 模型生成的文件。只支持 data / url */
type LanguageModelV4File = {
  type: 'file';
  mediaType: string;
  data:
    | { type: 'data'; data: Uint8Array | string }
    | { type: 'url'; url: URL };
  providerMetadata?: SharedV4ProviderMetadata;
};

/** 推理过程中生成的文件。只支持 data / url */
type LanguageModelV4ReasoningFile = {
  type: 'reasoning-file';
  mediaType: string;
  data:
    | { type: 'data'; data: Uint8Array | string }
    | { type: 'url'; url: URL };
  providerMetadata?: SharedV4ProviderMetadata;
};

/** 厂商对「厂商侧执行工具」发出的审批请求 */
type LanguageModelV4ToolApprovalRequest = {
  type: 'tool-approval-request';
  approvalId: string;
  toolCallId: string;
  providerMetadata?: SharedV4ProviderMetadata;
};

/** 生成时引用的来源 */
type LanguageModelV4Source =
  | {
      type: 'source';
      sourceType: 'url';
      id: string;
      url: string;
      title?: string;
      providerMetadata?: SharedV4ProviderMetadata;
    }
  | {
      type: 'source';
      sourceType: 'document';
      id: string;
      /** 如 application/pdf */
      mediaType: string;
      title: string;
      filename?: string;
      providerMetadata?: SharedV4ProviderMetadata;
    };

/** 模型生成的工具调用。注意 input 是字符串化 JSON，和 prompt 侧的对象不同 */
type LanguageModelV4ToolCall = {
  type: 'tool-call';
  toolCallId: string;
  toolName: string;
  /** 字符串化 JSON，需符合该工具的参数 schema */
  input: string;
  /** true 表示由厂商执行；未设或 false 表示由客户端执行 */
  providerExecuted?: boolean;
  /** 运行时定义的工具，例如厂商执行的 MCP 工具 */
  dynamic?: boolean;
  providerMetadata?: SharedV4ProviderMetadata;
};

/** 厂商已执行的工具结果 */
type LanguageModelV4ToolResult = {
  type: 'tool-result';
  toolCallId: string;
  toolName: string;
  /** JSON 可序列化对象 */
  result: NonNullable<JSONValue>;
  /** 该结果是否是错误 */
  isError?: boolean;
  /**
   * 是否为中间结果（如图片预览）。
   * 中间结果会互相替换，最终必须有一条非 preliminary 的结果。
   */
  preliminary?: boolean;
  /** 运行时定义的工具，例如厂商执行的 MCP 工具 */
  dynamic?: boolean;
  providerMetadata?: SharedV4ProviderMetadata;
};

type LanguageModelV4FinishReason = {
  /**
   * 统一结束原因：
   * stop 遇到停词；length 达到 token 上限；content-filter 内容过滤；
   * tool-calls 触发工具调用；error 出错；other 其他。
   */
  unified: 'stop' | 'length' | 'content-filter' | 'tool-calls' | 'error' | 'other';
  /** 厂商原始结束原因 */
  raw: string | undefined;
};

type LanguageModelV4Usage = {
  inputTokens: {
    /** 输入 token 总数 */
    total: number | undefined;
    /** 未命中缓存的输入 token */
    noCache: number | undefined;
    /** 读取缓存的输入 token */
    cacheRead: number | undefined;
    /** 写入缓存的输入 token */
    cacheWrite: number | undefined;
  };
  outputTokens: {
    /** 输出 token 总数 */
    total: number | undefined;
    /** 文本 token */
    text: number | undefined;
    /** 推理 token */
    reasoning: number | undefined;
  };
  /** 厂商原始 usage，可能含标准字段以外的信息 */
  raw?: JSONObject;
};

interface LanguageModelV4ResponseMetadata {
  /** 厂商返回的响应 ID */
  id?: string;
  /** 厂商返回的响应开始时间 */
  timestamp?: Date;
  /** 实际生成用的模型 ID */
  modelId?: string;
}

// ---------------------------------------------------------------------------
// 流式结果
// ---------------------------------------------------------------------------

type LanguageModelV4StreamResult = {
  stream: ReadableStream<LanguageModelV4StreamPart>;
  request?: { body?: unknown };
  response?: { headers?: SharedV4Headers };
};

type LanguageModelV4StreamPart =
  // 文本块
  | { type: 'text-start'; id: string; providerMetadata?: SharedV4ProviderMetadata }
  | { type: 'text-delta'; id: string; delta: string; providerMetadata?: SharedV4ProviderMetadata }
  | { type: 'text-end'; id: string; providerMetadata?: SharedV4ProviderMetadata }
  // 推理块
  | { type: 'reasoning-start'; id: string; providerMetadata?: SharedV4ProviderMetadata }
  | { type: 'reasoning-delta'; id: string; delta: string; providerMetadata?: SharedV4ProviderMetadata }
  | { type: 'reasoning-end'; id: string; providerMetadata?: SharedV4ProviderMetadata }
  // 工具入参流
  | {
      type: 'tool-input-start';
      id: string;
      toolName: string;
      providerExecuted?: boolean;
      dynamic?: boolean;
      title?: string;
      providerMetadata?: SharedV4ProviderMetadata;
    }
  | { type: 'tool-input-delta'; id: string; delta: string; providerMetadata?: SharedV4ProviderMetadata }
  | { type: 'tool-input-end'; id: string; providerMetadata?: SharedV4ProviderMetadata }
  | LanguageModelV4ToolApprovalRequest
  | LanguageModelV4ToolCall
  | LanguageModelV4ToolResult
  | LanguageModelV4CustomContent
  | LanguageModelV4File
  | LanguageModelV4ReasoningFile
  | LanguageModelV4Source
  // 流开始，带本次调用的警告
  | { type: 'stream-start'; warnings: Array<SharedV4Warning> }
  // 响应元数据，就绪后单独发一次
  | ({ type: 'response-metadata' } & LanguageModelV4ResponseMetadata)
  // 流结束
  | {
      type: 'finish';
      usage: LanguageModelV4Usage;
      finishReason: LanguageModelV4FinishReason;
      providerMetadata?: SharedV4ProviderMetadata;
    }
  // includeRawChunks 开启时的厂商原始 chunk
  | { type: 'raw'; rawValue: unknown }
  // 流式错误，可以出现多次
  | { type: 'error'; error: unknown };
```
