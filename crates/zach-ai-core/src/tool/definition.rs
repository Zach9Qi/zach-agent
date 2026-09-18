//! 工具定义与调用策略

use crate::options::ProviderOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 客户端定义的标准函数工具
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionTool {
    /// 工具名称（单次调用内唯一）
    pub name: String,
    /// 给模型看的工具用途说明
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 符合 JSON Schema Draft 7 的参数规范
    pub input_schema: Value,
    /// 可选输入示例
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_examples: Option<Vec<Value>>,
    /// 严格模式（支持的厂商会强校验 schema 合法性）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// 是否建议支持的厂商（如 Anthropic）使用服务端延迟加载模式
    ///
    /// 若设为 `true`，在适配支持该特性的厂商（如 Anthropic 的 `defer_loading`）时，
    /// 网关会将该工具暂缓注入初始 Prompt 上下文，待模型通过搜索工具按需检索后再动态加载。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defer_loading: Option<bool>,
    /// 厂商专有配置项
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
}

impl FunctionTool {
    /// 构造新的函数工具
    pub fn new(name: impl Into<String>, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            input_schema,
            input_examples: None,
            strict: None,
            defer_loading: None,
            provider_options: None,
        }
    }

    /// 设置工具描述
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 设置严格模式
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    /// 设置延迟加载模式
    pub fn with_defer_loading(mut self, defer_loading: bool) -> Self {
        self.defer_loading = Some(defer_loading);
        self
    }

    /// 设置输入示例
    pub fn with_input_examples(mut self, input_examples: Vec<Value>) -> Self {
        self.input_examples = Some(input_examples);
        self
    }

    /// 设置厂商专有配置项
    pub fn with_provider_options(mut self, provider_options: ProviderOptions) -> Self {
        self.provider_options = Some(provider_options);
        self
    }
}

/// 某个厂商特有的原生工具（如 OpenAI Web Search、Anthropic Bash/MCP）
///
/// 输入/输出 schema 由厂商定义，部分会在厂商侧执行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderTool {
    /// 格式：`<provider-id>.<unique-tool-name>`
    pub id: String,
    /// 工具对外名称
    pub name: String,
    /// 配置参数，必须符合该厂商对该工具的约定
    pub args: Value,
}

impl ProviderTool {
    /// 构造新的厂商原生工具
    pub fn new(id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            args,
        }
    }
}

/// 可用工具枚举（对应底层大模型协议的 ToolDefinition）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolDefinition {
    /// 客户端定义的标准函数工具
    Function(FunctionTool),
    /// 厂商特有的云端原生工具
    Provider(ProviderTool),
}

impl ToolDefinition {
    /// 获取工具名称
    pub fn name(&self) -> &str {
        match self {
            Self::Function(tool) => &tool.name,
            Self::Provider(tool) => &tool.name,
        }
    }

    /// 获取工具描述（仅 Function 具备）
    pub fn description(&self) -> Option<&str> {
        match self {
            Self::Function(tool) => tool.description.as_deref(),
            Self::Provider(_) => None,
        }
    }

    /// 是否为标准函数工具
    pub fn is_function(&self) -> bool {
        matches!(self, Self::Function(_))
    }

    /// 是否为厂商原生工具
    pub fn is_provider(&self) -> bool {
        matches!(self, Self::Provider(_))
    }

    /// 获取内部 FunctionTool 引用（若为 Function）
    pub fn as_function(&self) -> Option<&FunctionTool> {
        match self {
            Self::Function(tool) => Some(tool),
            Self::Provider(_) => None,
        }
    }

    /// 获取内部 ProviderTool 引用（若为 Provider）
    pub fn as_provider(&self) -> Option<&ProviderTool> {
        match self {
            Self::Provider(tool) => Some(tool),
            Self::Function(_) => None,
        }
    }
}

impl From<FunctionTool> for ToolDefinition {
    fn from(tool: FunctionTool) -> Self {
        Self::Function(tool)
    }
}

impl From<ProviderTool> for ToolDefinition {
    fn from(tool: ProviderTool) -> Self {
        Self::Provider(tool)
    }
}

/// 工具调用策略
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// 模型自主决定是否调用工具（默认）
    Auto,
    /// 禁止调用任何工具
    None,
    /// 必须调用至少一个可用工具
    Required,
    /// 必须调用指定名称的工具
    Tool { tool_name: String },
}

impl ToolChoice {
    /// 便捷构造指定特定工具的策略
    pub fn specific(tool_name: impl Into<String>) -> Self {
        Self::Tool {
            tool_name: tool_name.into(),
        }
    }
}
