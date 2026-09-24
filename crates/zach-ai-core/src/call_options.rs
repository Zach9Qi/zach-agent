//! 模型调用选项与超参数配置

use crate::options::ProviderOptions;
use crate::prompt::Prompt;
use crate::tool::{ToolChoice, ToolDefinition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 期望的模型响应输出格式
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// 普通自由文本
    Text,
    /// 结构化 JSON 输出
    Json {
        /// 期望输出符合的 JSON Schema
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Value>,
        /// Schema 名称（部分厂商用于进一步提示模型）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Schema 作用描述
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl ResponseFormat {
    /// 便捷构造 JSON Schema 约束格式
    pub fn json_schema(schema: Value) -> Self {
        Self::Json {
            schema: Some(schema),
            name: None,
            description: None,
        }
    }
}

/// 模型思考/推理强度预设档位
///
/// 序列化值使用 `snake_case`（如 `provider_default`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// 厂商与模型默认档位
    ProviderDefault,
    /// 彻底关闭推理思考
    None,
    /// 最低思考强度
    Minimal,
    /// 低思考强度
    Low,
    /// 中等思考强度
    Medium,
    /// 高思考强度
    High,
    /// 极高思考强度
    Xhigh,
    /// 最高思考强度（厂商提供的上限档位，如 DeepSeek、Claude 的 `max`）
    Max,
}

/// 统一调用入参（发送给具体厂商前所持有的标准形态）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallOptions {
    /// 标准化提示词历史
    pub prompt: Prompt,
    /// 最大生成 token 数量
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// 采样温度（0.0 ~ 2.0，依厂商和模型而定）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// 采样 Top-P（Nucleus sampling）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// 采样 Top-K
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// 存在惩罚（降低重复出现已提到主题的倾向）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    /// 频率惩罚（降低逐字逐句重复的倾向）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    /// 停止词序列列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    /// 期望的响应输出格式
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    /// 随机采样种子
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    /// 本次调用可用的工具列表
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// 工具选择策略
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// 推理思考强度档位
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningEffort>,
    /// 流式调用时是否将厂商底层原始分块作为 Raw 事件透出
    #[serde(default)]
    pub include_raw_chunks: bool,
    /// 额外 HTTP 请求头（仅对基于 HTTP 通信的厂商生效）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
    /// 厂商专有配置参数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl CallOptions {
    /// 构造以指定 Prompt 为核心的调用配置
    pub fn new(prompt: impl Into<Prompt>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Default::default()
        }
    }

    /// 设置采样温度
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// 设置最大生成 Token
    pub fn with_max_output_tokens(mut self, max_tokens: u32) -> Self {
        self.max_output_tokens = Some(max_tokens);
        self
    }

    /// 设置可用工具列表
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// 设置工具选择策略
    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    /// 设置推理思考档位
    pub fn with_reasoning(mut self, reasoning: ReasoningEffort) -> Self {
        self.reasoning = Some(reasoning);
        self
    }
}
