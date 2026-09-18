//! Token 消耗统计数据结构

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Token 消耗统计
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// 输入侧 Token
    pub input_tokens: InputTokenUsage,
    /// 输出侧 Token
    pub output_tokens: OutputTokenUsage,
    /// 厂商原始 usage 数据（可能包含厂商专属的额外字段）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// 输入 Token 详细分解
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputTokenUsage {
    /// 输入 Token 总数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// 未命中缓存的输入 Token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<u64>,
    /// 命中并读取缓存的输入 Token 数（Prompt Caching）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    /// 写入缓存的输入 Token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
}

/// 输出 Token 详细分解
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OutputTokenUsage {
    /// 输出 Token 总数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// 文本正文消耗的 Token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<u64>,
    /// 思考链/推理过程消耗的 Token 数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u64>,
}

impl Usage {
    /// 便捷构造仅包含总输入与总输出的 Usage
    pub fn simple(input_total: u64, output_total: u64) -> Self {
        Self {
            input_tokens: InputTokenUsage {
                total: Some(input_total),
                ..Default::default()
            },
            output_tokens: OutputTokenUsage {
                total: Some(output_total),
                ..Default::default()
            },
            raw: None,
        }
    }
}
