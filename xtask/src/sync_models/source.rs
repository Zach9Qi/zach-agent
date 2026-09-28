//! models.dev `api.json` 的数据结构（只声明用到的字段）

use std::collections::HashMap;

use serde::Deserialize;

/// 厂商标识 → 厂商数据
pub(crate) type ModelsDevCatalog = HashMap<String, ModelsDevProvider>;

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevProvider {
    #[serde(default)]
    pub models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevModel {
    pub name: Option<String>,
    #[serde(default)]
    pub tool_call: bool,
    #[serde(default)]
    pub structured_output: bool,
    pub temperature: Option<bool>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub reasoning_options: Vec<ReasoningOption>,
    pub status: Option<String>,
    #[serde(default)]
    pub modalities: ModelsDevModalities,
    #[serde(default)]
    pub limit: ModelsDevLimit,
    pub cost: Option<ModelsDevCost>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ModelsDevModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ModelsDevLimit {
    pub context: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    #[serde(default)]
    pub tiers: Vec<ModelsDevCostTier>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ModelsDevCostTier {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub tier: Option<TierCondition>,
}

/// 阶梯生效条件，如 `{ "type": "context", "size": 200000 }`
#[derive(Debug, Deserialize)]
pub(crate) struct TierCondition {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub size: Option<u64>,
}

/// 推理选项；未识别的类型忽略
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReasoningOption {
    Toggle,
    Effort {
        #[serde(default)]
        values: Vec<Option<String>>,
    },
    #[serde(other)]
    Other,
}
