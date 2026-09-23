//! Agent 运行时语义化事件枚举定义
//!
//! 提供在 Agent 运行周期、工具调度、文本流式生成与人机交互过程中完整的语义化事件流。
//! 能够对齐并完整覆盖前端流式规范（包括 Vercel AI SDK 的 `UIMessageChunk` 等）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zach_ai_core::{FileData, FinishReason, ProviderMetadata, ToolResultOutput};

/// Agent 运行过程中的核心语义化事件
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEvent {
    // -----------------------------------------------------------------------
    // 1. 运行级生命周期（Run Lifecycle）
    // -----------------------------------------------------------------------
    /// 整个 Agent 运行启动
    #[serde(rename_all = "camelCase")]
    RunStart {
        /// 本次运行的全局唯一标识
        run_id: String,
        /// 关联的消息 ID（如前端 Assistant 消息 ID）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        /// 本次运行初始化时传入的消息级元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_metadata: Option<Value>,
    },
    /// 整个 Agent 运行正常结束
    #[serde(rename_all = "camelCase")]
    RunFinish {
        /// 运行结束原因（如正常停止 stop、工具调用 tool_calls 等）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        finish_reason: Option<FinishReason>,
        /// 运行完成时更新或附加的消息元数据（如全局 Token 用量 usage）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_metadata: Option<Value>,
    },
    /// 运行被主动中断或取消
    #[serde(rename_all = "camelCase")]
    RunAbort {
        /// 中断的具体原因说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// 发生不可恢复的致命运行时错误
    #[serde(rename_all = "camelCase")]
    RunError {
        /// 错误详情描述文本
        error_text: String,
    },

    // -----------------------------------------------------------------------
    // 2. 步骤级生命周期（Step Lifecycle）
    // -----------------------------------------------------------------------
    /// 单轮 Step（一次思考或模型调用）开始
    #[serde(rename_all = "camelCase")]
    StepStart {
        /// 当前执行的步骤轮次索引（从 0 开始）
        step_index: usize,
    },
    /// 当前 Step 执行结束
    #[serde(rename_all = "camelCase")]
    StepFinish {
        /// 刚刚结束的步骤轮次索引
        step_index: usize,
    },
    /// 重置或重试当前 Step（语义等价于 reset-step，通知客户端丢弃当前步骤未完成的内容）
    #[serde(rename_all = "camelCase")]
    StepRetry {
        /// 触发重置/重试的步骤轮次索引
        step_index: usize,
        /// 重置或重试的原因说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    // -----------------------------------------------------------------------
    // 3. 文本生成（Text Streaming）
    // -----------------------------------------------------------------------
    /// 新文本段落开始生成
    #[serde(rename_all = "camelCase")]
    TextStart {
        /// 文本块的全局唯一标识 ID
        id: String,
        /// 模型厂商返回的文本段落初始元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 模型生成的正文文本增量片段
    #[serde(rename_all = "camelCase")]
    TextDelta {
        /// 文本块的全局唯一标识 ID
        id: String,
        /// 文本增量字符内容
        delta: String,
        /// 模型厂商随增量片段附带的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 当前文本段落生成结束
    #[serde(rename_all = "camelCase")]
    TextFinish {
        /// 文本块的全局唯一标识 ID
        id: String,
        /// 模型厂商在文本结束时附加的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 4. 推理/思考链（Reasoning / Thinking）
    // -----------------------------------------------------------------------
    /// 思考链生成开始
    #[serde(rename_all = "camelCase")]
    ReasoningStart {
        /// 思考链块的全局唯一标识 ID
        id: String,
        /// 模型厂商返回的思考链初始元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链文字内容增量
    #[serde(rename_all = "camelCase")]
    ReasoningDelta {
        /// 思考链块的全局唯一标识 ID
        id: String,
        /// 思考过程文本增量片段
        delta: String,
        /// 模型厂商随思考片段附带的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考链生成结束
    #[serde(rename_all = "camelCase")]
    ReasoningFinish {
        /// 思考链块的全局唯一标识 ID
        id: String,
        /// 模型厂商在思考结束时附加的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 思考过程中由模型生成的文件附件
    #[serde(rename_all = "camelCase")]
    ReasoningFile {
        /// 文件的媒体类型（MIME Type，如 image/png）
        media_type: String,
        /// 文件的远程可访问 URL
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// 文件的内联二进制/Base64 数据实体
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<FileData>,
        /// 模型厂商附加元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 5. 工具调用（入参）
    // -----------------------------------------------------------------------
    /// 工具调用开始流式接收入参
    #[serde(rename_all = "camelCase")]
    ToolInputStart {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 目标工具名称
        tool_name: String,
        /// 是否为客户端/运行时动态工具
        #[serde(default)]
        dynamic: bool,
        /// 是否由模型供应商在服务端直接代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 工具在前端 UI 渲染时的友好标题
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 工具本身的自定义配置/元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_metadata: Option<Value>,
        /// 模型厂商针对此工具调用的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具参数 JSON 字符串增量片段
    #[serde(rename_all = "camelCase")]
    ToolInputDelta {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 入参 JSON 字符串片段增量
        input_text_delta: String,
    },
    /// 工具入参完整解析/校验通过并可用
    #[serde(rename_all = "camelCase")]
    ToolInputAvailable {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 目标工具名称
        tool_name: String,
        /// 校验通过的结构化工具入参（JSON Value）
        input: Value,
        /// 是否为客户端/运行时动态工具
        #[serde(default)]
        dynamic: bool,
        /// 是否由模型供应商在服务端直接代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 工具在前端 UI 渲染时的友好标题
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 工具本身的自定义配置/元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_metadata: Option<Value>,
        /// 模型厂商针对此工具调用的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具入参解析或校验出错
    #[serde(rename_all = "camelCase")]
    ToolInputError {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 目标工具名称
        tool_name: String,
        /// 出错时的原始入参内容或文本
        input: String,
        /// 参数解析或校验失败的错误描述
        error_text: String,
        /// 是否为客户端/运行时动态工具
        #[serde(default)]
        dynamic: bool,
        /// 是否由模型供应商在服务端直接代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 工具在前端 UI 渲染时的友好标题
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 工具本身的自定义配置/元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_metadata: Option<Value>,
        /// 模型厂商针对此工具调用的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 6. 工具执行（结果）
    // -----------------------------------------------------------------------
    /// 工具执行完成并返回结果（支持流式中间结果）
    #[serde(rename_all = "camelCase")]
    ToolOutputAvailable {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 工具执行的结构化输出内容
        output: ToolResultOutput,
        /// 是否为流式执行过程中的中间结果（非最终态）
        #[serde(default)]
        preliminary: bool,
        /// 是否为动态工具
        #[serde(default)]
        dynamic: bool,
        /// 是否由模型供应商在服务端直接代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 工具本身的自定义配置/元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_metadata: Option<Value>,
        /// 模型服务商附带的工具执行结果元数据（如耗时、厂商自定义用量）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具执行过程中抛出异常或失败
    #[serde(rename_all = "camelCase")]
    ToolOutputError {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 工具执行抛出的错误信息文本
        error_text: String,
        /// 是否为动态工具
        #[serde(default)]
        dynamic: bool,
        /// 是否由模型供应商在服务端直接代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 工具本身的自定义配置/元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_metadata: Option<Value>,
        /// 模型服务商附带的工具执行元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 工具执行被安全策略或人工审批拒绝
    #[serde(rename_all = "camelCase")]
    ToolOutputDenied {
        /// 该工具调用的唯一标识 ID
        tool_call_id: String,
        /// 拒绝原因说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },

    // -----------------------------------------------------------------------
    // 7. 人机交互/审批（Human-in-the-loop）
    // -----------------------------------------------------------------------
    /// 请求用户或客户端对特定工具调用进行人工审批
    #[serde(rename_all = "camelCase")]
    ToolApprovalRequest {
        /// 审批请求的唯一跟踪 ID
        approval_id: String,
        /// 关联的待审批工具调用 ID
        tool_call_id: String,
        /// 待审批的工具名称
        tool_name: String,
        /// 待审批的结构化工具入参
        input: Value,
        /// 结构化审批上下文描述符（如风险评级、Diff 预览、权限清单）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_descriptor: Option<Value>,
        /// 触发人工审批的原因说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// 是否由安全策略规则自动化触发
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_automatic: Option<bool>,
        /// 安全策略校验签名或防篡改验签串
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// 工具审批响应结果通知
    #[serde(rename_all = "camelCase")]
    ToolApprovalResponse {
        /// 对应的审批请求唯一 ID
        approval_id: String,
        /// 审批决策结果（true 为批准执行，false 为拒绝）
        approved: bool,
        /// 审批批注或拒绝原因说明
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// 审批后是否交由模型厂商代为执行
        #[serde(default)]
        provider_executed: bool,
        /// 伴随审批操作由模型或安全系统产生的元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 8. 知识引用与文件（Sources & Files）
    // -----------------------------------------------------------------------
    /// 模型引用或检索到的外部网页来源
    #[serde(rename_all = "camelCase")]
    SourceUrl {
        /// 来源引用的唯一标识 ID
        source_id: String,
        /// 网页完整 URL 地址
        url: String,
        /// 网页或来源标题
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        /// 模型或检索工具附带的来源元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 模型引用的文档切片或知识库资料
    #[serde(rename_all = "camelCase")]
    SourceDocument {
        /// 来源引用的唯一标识 ID
        source_id: String,
        /// 文档切片的媒体类型（MIME Type）
        media_type: String,
        /// 文档或切片标题
        title: String,
        /// 原始文件名
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// 模型或检索工具附带的来源元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
    /// 模型产出的通用文件附件（图片、文档等）
    #[serde(rename_all = "camelCase")]
    FileAttachment {
        /// 文件的媒体类型（MIME Type）
        media_type: String,
        /// 文件的远程可访问 URL 地址
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        /// 文件的内联二进制/Base64 数据实体
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<FileData>,
        /// 模型厂商生成的附件元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },

    // -----------------------------------------------------------------------
    // 9. 自定义扩展与元数据（Custom & Metadata）
    // -----------------------------------------------------------------------
    /// 针对整条消息的元数据增量更新
    #[serde(rename_all = "camelCase")]
    MessageMetadata {
        /// 待合并到当前消息的元数据对象
        message_metadata: Value,
    },
    /// 业务自定义数据通道（持久或瞬态）
    #[serde(rename_all = "camelCase")]
    CustomData {
        /// 业务通道名称（对应 AI SDK `data-${NAME}` 中的 NAME）
        channel: String,
        /// 数据部件唯一 ID（用于前端依据 ID 更新部件）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// 业务自定义载荷数据
        data: Value,
        /// 是否为瞬态数据（true 表示仅触发前端事件，不持久化保存到消息部件历史中）
        #[serde(default)]
        transient: bool,
    },
    /// Provider 或特定协议的扩展事件
    #[serde(rename_all = "camelCase")]
    Custom {
        /// 扩展事件类型标识（如 "namespace.event"）
        kind: String,
        /// 扩展事件载荷数据
        data: Value,
        /// 伴随扩展事件的模型厂商元数据
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_metadata: Option<ProviderMetadata>,
    },
}
