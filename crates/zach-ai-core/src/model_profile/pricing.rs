//! 模型计费单价与费用计算

use crate::response::Usage;
use serde::{Deserialize, Serialize};

const TOKENS_PER_UNIT: f64 = 1_000_000.0;

/// 一组计费单价（美元 / 百万 Token）
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PricingRates {
    /// 未命中缓存的输入单价
    pub input: f64,
    /// 输出单价（含推理 Token）
    pub output: f64,
    /// 缓存读取单价；未知时按 `input` 计费
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// 缓存写入单价；未知时按 `input` 计费
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// 阶梯价：整次请求的输入 Token 超过阈值时，全部改用该档单价
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PricingTier {
    /// 输入 Token 总数（含缓存部分）超过该值时生效
    pub input_tokens_above: u64,
    /// 该档单价
    #[serde(flatten)]
    pub rates: PricingRates,
}

/// 模型计费单价
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    /// 基础单价
    #[serde(flatten)]
    pub base: PricingRates,
    /// 阶梯价；命中多档时取阈值最高的一档
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tiers: Vec<PricingTier>,
}

/// 单次调用的费用明细（美元）
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// 未命中缓存的输入费用
    pub input: f64,
    /// 输出费用
    pub output: f64,
    /// 缓存读取费用
    pub cache_read: f64,
    /// 缓存写入费用
    pub cache_write: f64,
    /// 合计
    pub total: f64,
}

impl ModelPricing {
    /// 以基础单价构造（无缓存单价、无阶梯）
    pub fn new(input: f64, output: f64) -> Self {
        Self {
            base: PricingRates {
                input,
                output,
                cache_read: None,
                cache_write: None,
            },
            tiers: Vec::new(),
        }
    }

    /// 按输入 Token 总数选出生效的单价
    pub fn rates_for(&self, input_tokens: u64) -> &PricingRates {
        self.tiers
            .iter()
            .filter(|tier| input_tokens > tier.input_tokens_above)
            .max_by_key(|tier| tier.input_tokens_above)
            .map_or(&self.base, |tier| &tier.rates)
    }

    /// 计算一次调用的费用
    pub fn cost(&self, usage: &Usage) -> CostBreakdown {
        let input = &usage.input_tokens;
        let cache_read = input.cache_read.unwrap_or(0);
        let cache_write = input.cache_write.unwrap_or(0);
        // `total` 含缓存部分，缺少 `no_cache` 时由它反推
        let no_cache = input.no_cache.unwrap_or_else(|| {
            input
                .total
                .unwrap_or(0)
                .saturating_sub(cache_read + cache_write)
        });

        let output_tokens = &usage.output_tokens;
        let output = output_tokens.total.unwrap_or_else(|| {
            output_tokens.text.unwrap_or(0) + output_tokens.reasoning.unwrap_or(0)
        });

        let rates = self.rates_for(no_cache + cache_read + cache_write);
        let price = |tokens: u64, rate: f64| tokens as f64 * rate / TOKENS_PER_UNIT;

        let input_cost = price(no_cache, rates.input);
        let output_cost = price(output, rates.output);
        let cache_read_cost = price(cache_read, rates.cache_read.unwrap_or(rates.input));
        let cache_write_cost = price(cache_write, rates.cache_write.unwrap_or(rates.input));

        CostBreakdown {
            input: input_cost,
            output: output_cost,
            cache_read: cache_read_cost,
            cache_write: cache_write_cost,
            total: input_cost + output_cost + cache_read_cost + cache_write_cost,
        }
    }
}
