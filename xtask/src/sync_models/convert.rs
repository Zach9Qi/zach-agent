//! models.dev 模型 → `ModelProfile` 的字段映射

use zach_ai_core::{
    Modalities, Modality, ModelLimits, ModelPricing, ModelProfile, ModelStatus, PricingRates,
    PricingTier, ReasoningEffort, ReasoningProfile,
};

use super::source::{ModelsDevCost, ModelsDevModel, ReasoningOption};

/// 缺省的 Token 上限（models.dev 未提供时的保守值）
const FALLBACK_LIMIT: u64 = 4096;

pub(crate) fn to_profile(provider: &str, id: &str, model: &ModelsDevModel) -> ModelProfile {
    let context_window = model.limit.context.unwrap_or(FALLBACK_LIMIT);
    ModelProfile {
        provider: provider.to_owned(),
        id: id.to_owned(),
        name: model.name.clone().unwrap_or_else(|| id.to_owned()),
        modalities: Modalities {
            input: to_modalities(&model.modalities.input),
            output: to_modalities(&model.modalities.output),
        },
        tool_call: model.tool_call,
        structured_output: model.structured_output,
        temperature: model.temperature.unwrap_or(true),
        reasoning: model
            .reasoning
            .then(|| to_reasoning(&model.reasoning_options)),
        limits: ModelLimits {
            context_window,
            max_output_tokens: model.limit.output.unwrap_or(FALLBACK_LIMIT),
            max_input_tokens: model.limit.input.filter(|&input| input < context_window),
        },
        pricing: model.cost.as_ref().map(to_pricing),
        status: to_status(model.status.as_deref()),
    }
}

/// 未知模态直接丢弃；为空时退回仅文本
fn to_modalities(values: &[String]) -> Vec<Modality> {
    let modalities: Vec<Modality> = values
        .iter()
        .filter_map(|value| match value.as_str() {
            "text" => Some(Modality::Text),
            "image" => Some(Modality::Image),
            "pdf" => Some(Modality::Pdf),
            "audio" => Some(Modality::Audio),
            "video" => Some(Modality::Video),
            _ => None,
        })
        .collect();
    if modalities.is_empty() {
        vec![Modality::Text]
    } else {
        modalities
    }
}

fn to_reasoning(options: &[ReasoningOption]) -> ReasoningProfile {
    let mut profile = ReasoningProfile::default();
    for option in options {
        match option {
            ReasoningOption::Toggle => profile.can_disable = true,
            ReasoningOption::Effort { values } => {
                for value in values.iter().flatten() {
                    let effort = match value.as_str() {
                        "none" => {
                            profile.can_disable = true;
                            continue;
                        }
                        "minimal" => ReasoningEffort::Minimal,
                        "low" => ReasoningEffort::Low,
                        "medium" => ReasoningEffort::Medium,
                        "high" => ReasoningEffort::High,
                        "xhigh" => ReasoningEffort::Xhigh,
                        "max" => ReasoningEffort::Max,
                        _ => continue,
                    };
                    if !profile.efforts.contains(&effort) {
                        profile.efforts.push(effort);
                    }
                }
            }
            ReasoningOption::Other => {}
        }
    }
    profile
}

fn to_pricing(cost: &ModelsDevCost) -> ModelPricing {
    let tiers = cost
        .tiers
        .iter()
        // 目前只识别按上下文长度分档的阶梯价
        .filter_map(|tier| {
            let condition = tier.tier.as_ref()?;
            if condition.kind.as_deref() != Some("context") {
                return None;
            }
            Some(PricingTier {
                input_tokens_above: condition.size?,
                rates: PricingRates {
                    input: tier.input,
                    output: tier.output,
                    cache_read: tier.cache_read,
                    cache_write: tier.cache_write,
                },
            })
        })
        .collect();
    ModelPricing {
        base: PricingRates {
            input: cost.input,
            output: cost.output,
            cache_read: cost.cache_read,
            cache_write: cost.cache_write,
        },
        tiers,
    }
}

fn to_status(status: Option<&str>) -> ModelStatus {
    match status {
        Some("alpha") => ModelStatus::Alpha,
        Some("beta") => ModelStatus::Beta,
        Some("deprecated") => ModelStatus::Deprecated,
        _ => ModelStatus::Active,
    }
}
