//! 模型档案（ModelProfile）：模型的静态能力、限制与计费信息
//!
//! 只描述"模型是什么、能做什么、多少钱"，与请求协议无关；
//! 接入地址、请求头、协议类型等连接配置由实现层（`zach-ai`）负责。

pub mod pricing;

pub use pricing::{CostBreakdown, ModelPricing, PricingRates, PricingTier};

use crate::call_options::ReasoningEffort;
use crate::response::Usage;
use serde::{Deserialize, Serialize};

/// 模型档案
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelProfile {
    /// 厂商标识，与 [`crate::LanguageModel::provider`] 一致
    pub provider: String,
    /// 模型标识，与 [`crate::LanguageModel::model_id`] 一致
    pub id: String,
    /// 展示名称
    pub name: String,
    /// 输入输出模态
    #[serde(default)]
    pub modalities: Modalities,
    /// 是否支持工具调用
    #[serde(default)]
    pub tool_call: bool,
    /// 是否支持按 JSON Schema 约束的结构化输出
    #[serde(default)]
    pub structured_output: bool,
    /// 是否接受 `temperature` 采样参数（部分推理模型会拒绝）
    #[serde(default = "default_true")]
    pub temperature: bool,
    /// 推理能力；`None` 表示不支持推理
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningProfile>,
    /// Token 限制
    pub limits: ModelLimits,
    /// 计费单价；`None` 表示价格未知（与免费模型的零单价区分）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<ModelPricing>,
    /// 生命周期状态
    #[serde(default)]
    pub status: ModelStatus,
}

fn default_true() -> bool {
    true
}

impl ModelProfile {
    /// 以最少必要信息构造档案，其余能力取保守默认值（仅文本、不支持工具与推理、价格未知）
    pub fn new(
        provider: impl Into<String>,
        id: impl Into<String>,
        context_window: u64,
        max_output_tokens: u64,
    ) -> Self {
        let id = id.into();
        Self {
            provider: provider.into(),
            name: id.clone(),
            id,
            modalities: Modalities::default(),
            tool_call: false,
            structured_output: false,
            temperature: true,
            reasoning: None,
            limits: ModelLimits {
                context_window,
                max_output_tokens,
                max_input_tokens: None,
            },
            pricing: None,
            status: ModelStatus::default(),
        }
    }

    /// 是否支持推理
    pub fn supports_reasoning(&self) -> bool {
        self.reasoning.is_some()
    }

    /// 是否支持指定的推理档位；不支持推理的模型只接受 `ProviderDefault`
    pub fn supports_reasoning_effort(&self, effort: ReasoningEffort) -> bool {
        match &self.reasoning {
            Some(reasoning) => reasoning.supports(effort),
            None => effort == ReasoningEffort::ProviderDefault,
        }
    }

    /// 按单价计算一次调用的费用；价格未知时返回 `None`
    pub fn cost(&self, usage: &Usage) -> Option<CostBreakdown> {
        self.pricing.as_ref().map(|pricing| pricing.cost(usage))
    }
}

/// 模态类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    /// 文本
    Text,
    /// 图片
    Image,
    /// PDF 文档
    Pdf,
    /// 音频
    Audio,
    /// 视频
    Video,
}

/// 输入输出模态集合
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Modalities {
    /// 可接受的输入模态
    pub input: Vec<Modality>,
    /// 可产出的输出模态
    pub output: Vec<Modality>,
}

impl Default for Modalities {
    fn default() -> Self {
        Self {
            input: vec![Modality::Text],
            output: vec![Modality::Text],
        }
    }
}

impl Modalities {
    /// 是否接受指定输入模态
    pub fn accepts(&self, modality: Modality) -> bool {
        self.input.contains(&modality)
    }

    /// 是否能产出指定输出模态
    pub fn produces(&self, modality: Modality) -> bool {
        self.output.contains(&modality)
    }
}

/// Token 限制
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelLimits {
    /// 上下文窗口（输入与输出 Token 合计上限）
    pub context_window: u64,
    /// 单次最大输出 Token
    pub max_output_tokens: u64,
    /// 单独的输入 Token 上限（小于上下文窗口时才有意义，如部分 OpenAI 模型）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
}

/// 推理能力描述
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReasoningProfile {
    /// 支持的推理档位（不含 `ProviderDefault` 与 `None`）；为空表示档位未知，不做限制
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<ReasoningEffort>,
    /// 是否允许关闭推理（对应 [`ReasoningEffort::None`]）
    #[serde(default)]
    pub can_disable: bool,
}

impl ReasoningProfile {
    /// 是否支持指定的推理档位
    pub fn supports(&self, effort: ReasoningEffort) -> bool {
        match effort {
            ReasoningEffort::ProviderDefault => true,
            ReasoningEffort::None => self.can_disable,
            other => self.efforts.is_empty() || self.efforts.contains(&other),
        }
    }
}

/// 模型生命周期状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    /// 正式可用
    #[default]
    Active,
    /// 内测
    Alpha,
    /// 公测
    Beta,
    /// 已弃用，可能随时下线
    Deprecated,
}
