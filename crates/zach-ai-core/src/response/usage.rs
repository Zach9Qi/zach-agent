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

    /// 把 `other` 逐字段累加进自身，用于汇总多次调用的用量
    ///
    /// 只有双方都为 `None` 的字段才保持 `None`；`raw` 是厂商原始数据，不参与累加、保持不变。
    pub fn add(&mut self, other: &Usage) {
        add_opt(&mut self.input_tokens.total, other.input_tokens.total);
        add_opt(&mut self.input_tokens.no_cache, other.input_tokens.no_cache);
        add_opt(
            &mut self.input_tokens.cache_read,
            other.input_tokens.cache_read,
        );
        add_opt(
            &mut self.input_tokens.cache_write,
            other.input_tokens.cache_write,
        );
        add_opt(&mut self.output_tokens.total, other.output_tokens.total);
        add_opt(&mut self.output_tokens.text, other.output_tokens.text);
        add_opt(
            &mut self.output_tokens.reasoning,
            other.output_tokens.reasoning,
        );
    }
}

impl std::ops::AddAssign<&Usage> for Usage {
    fn add_assign(&mut self, other: &Usage) {
        self.add(other);
    }
}

fn add_opt(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0) + value);
    }
}
