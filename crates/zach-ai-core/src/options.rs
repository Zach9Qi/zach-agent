//! 厂商专有扩展选项与元数据

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 厂商专有入参字典。
/// 外层 key 是厂商标识（如 "openai"、"anthropic"），内层是厂商私有参数字段。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderOptions {
    pub inner: HashMap<String, Value>,
}

impl ProviderOptions {
    /// 创建空选项
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入特定厂商的专有选项
    pub fn insert<T: Serialize>(&mut self, provider: impl Into<String>, options: T) {
        if let Ok(val) = serde_json::to_value(options) {
            self.inner.insert(provider.into(), val);
        }
    }

    /// 获取并反序列化特定厂商的专有选项
    pub fn get<T: for<'de> Deserialize<'de>>(&self, provider: &str) -> Option<T> {
        self.inner
            .get(provider)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// 判断是否为空
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// 厂商专有出参/响应元数据
pub type ProviderMetadata = ProviderOptions;

/// 模型调用警告：不支持、兼容降级、弃用、其他
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelWarning {
    /// 模型或厂商原生不支持该功能
    Unsupported {
        feature: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// 功能降级兼容运行
    Compatibility {
        feature: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        details: Option<String>,
    },
    /// 使用了已弃用的设置项
    Deprecated { setting: String, message: String },
    /// 其他警告信息
    Other { message: String },
}
