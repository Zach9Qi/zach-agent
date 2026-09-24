//! 模型档案：JSON 缺省值、推理档位判断与费用计算。

use async_trait::async_trait;
use serde_json::json;
use zach_ai_core::{
    CallOptions, GenerateResult, InputTokenUsage, LanguageModel, LanguageModelStream, Modality,
    ModelError, ModelPricing, ModelProfile, ModelStatus, OutputTokenUsage, PricingRates,
    PricingTier, ReasoningEffort, ReasoningProfile, Usage,
};

struct StubModel {
    profile: Option<ModelProfile>,
}

#[async_trait]
impl LanguageModel for StubModel {
    fn provider(&self) -> &str {
        "stub"
    }

    fn model_id(&self) -> &str {
        "stub-model"
    }

    fn profile(&self) -> Option<&ModelProfile> {
        self.profile.as_ref()
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        unimplemented!("档案测试不发起调用")
    }

    async fn do_stream(&self, _options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        unimplemented!("档案测试不发起调用")
    }
}

struct BareModel;

#[async_trait]
impl LanguageModel for BareModel {
    fn provider(&self) -> &str {
        "bare"
    }

    fn model_id(&self) -> &str {
        "bare-model"
    }

    async fn do_generate(&self, _options: CallOptions) -> Result<GenerateResult, ModelError> {
        unimplemented!("档案测试不发起调用")
    }

    async fn do_stream(&self, _options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        unimplemented!("档案测试不发起调用")
    }
}

fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "期望 {expected}，实际 {actual}"
    );
}

fn usage(no_cache: Option<u64>, total: Option<u64>, cache_read: u64, output: u64) -> Usage {
    Usage {
        input_tokens: InputTokenUsage {
            total,
            no_cache,
            cache_read: Some(cache_read),
            cache_write: None,
        },
        output_tokens: OutputTokenUsage {
            total: Some(output),
            ..Default::default()
        },
        raw: None,
    }
}

#[test]
fn language_model_exposes_optional_profile() {
    let bare: Box<dyn LanguageModel> = Box::new(BareModel);
    assert!(bare.profile().is_none());

    let stub: Box<dyn LanguageModel> = Box::new(StubModel {
        profile: Some(ModelProfile::new("stub", "stub-model", 128_000, 16_384)),
    });
    let profile = stub.profile().expect("应返回档案");
    assert_eq!(profile.id, stub.model_id());
    assert_eq!(profile.limits.context_window, 128_000);
}

#[test]
fn minimal_json_uses_conservative_defaults() {
    let profile: ModelProfile = serde_json::from_value(json!({
        "provider": "local",
        "id": "qwen3",
        "name": "Qwen3",
        "limits": { "context_window": 32768, "max_output_tokens": 8192 }
    }))
    .unwrap();

    assert!(profile.modalities.accepts(Modality::Text));
    assert!(!profile.modalities.accepts(Modality::Image));
    assert!(!profile.tool_call);
    assert!(profile.temperature);
    assert!(!profile.supports_reasoning());
    assert_eq!(profile.limits.max_input_tokens, None);
    assert_eq!(profile.pricing, None);
    assert_eq!(profile.status, ModelStatus::Active);
    assert_eq!(profile.cost(&Usage::simple(1000, 1000)), None);
}

#[test]
fn full_profile_roundtrips_with_flattened_pricing() {
    let value = json!({
        "provider": "anthropic",
        "id": "claude-sonnet-4-5",
        "name": "Claude Sonnet 4.5",
        "modalities": { "input": ["text", "image", "pdf"], "output": ["text"] },
        "tool_call": true,
        "structured_output": true,
        "temperature": true,
        "reasoning": { "efforts": ["low", "medium", "high"], "can_disable": true },
        "limits": { "context_window": 1000000, "max_output_tokens": 64000 },
        "pricing": {
            "input": 3.0,
            "output": 15.0,
            "cache_read": 0.3,
            "cache_write": 3.75,
            "tiers": [
                { "input_tokens_above": 200000, "input": 6.0, "output": 22.5, "cache_read": 0.6, "cache_write": 7.5 }
            ]
        },
        "status": "active"
    });

    let profile: ModelProfile = serde_json::from_value(value.clone()).unwrap();
    assert!(profile.modalities.accepts(Modality::Pdf));
    assert_eq!(profile.pricing.as_ref().unwrap().tiers.len(), 1);
    assert_eq!(serde_json::to_value(&profile).unwrap(), value);
}

#[test]
fn status_and_modality_use_snake_case() {
    assert_eq!(
        serde_json::to_value(ModelStatus::Deprecated).unwrap(),
        json!("deprecated")
    );
    assert_eq!(serde_json::to_value(Modality::Pdf).unwrap(), json!("pdf"));
}

#[test]
fn reasoning_effort_support_follows_profile() {
    let mut profile = ModelProfile::new("deepseek", "deepseek-v4", 1_000_000, 384_000);
    assert!(profile.supports_reasoning_effort(ReasoningEffort::ProviderDefault));
    assert!(!profile.supports_reasoning_effort(ReasoningEffort::High));

    profile.reasoning = Some(ReasoningProfile {
        efforts: vec![
            ReasoningEffort::Low,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ],
        can_disable: true,
    });
    assert!(profile.supports_reasoning_effort(ReasoningEffort::Max));
    assert!(profile.supports_reasoning_effort(ReasoningEffort::None));
    assert!(!profile.supports_reasoning_effort(ReasoningEffort::Medium));

    let unknown_levels = ReasoningProfile::default();
    assert!(unknown_levels.supports(ReasoningEffort::Xhigh));
    assert!(!unknown_levels.supports(ReasoningEffort::None));
}

#[test]
fn cost_uses_no_cache_and_cache_read_rates() {
    let pricing = ModelPricing {
        base: PricingRates {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.3),
            cache_write: None,
        },
        tiers: Vec::new(),
    };

    let cost = pricing.cost(&usage(Some(1_000), Some(11_000), 10_000, 2_000));
    assert_close(cost.input, 0.003);
    assert_close(cost.cache_read, 0.003);
    assert_close(cost.output, 0.03);
    assert_close(cost.total, 0.036);
}

#[test]
fn cost_derives_no_cache_from_total_and_falls_back_to_input_rate() {
    let pricing = ModelPricing::new(2.0, 8.0);

    let cost = pricing.cost(&usage(None, Some(5_000), 4_000, 0));
    assert_close(cost.input, 0.002);
    assert_close(cost.cache_read, 0.008);
    assert_close(cost.total, 0.01);
}

#[test]
fn cost_sums_text_and_reasoning_when_output_total_missing() {
    let pricing = ModelPricing::new(0.0, 10.0);
    let usage = Usage {
        output_tokens: OutputTokenUsage {
            total: None,
            text: Some(300),
            reasoning: Some(700),
        },
        ..Default::default()
    };

    assert_close(pricing.cost(&usage).output, 0.01);
}

#[test]
fn highest_matching_tier_applies_to_whole_request() {
    let tier = |above: u64, input: f64| PricingTier {
        input_tokens_above: above,
        rates: PricingRates {
            input,
            output: 0.0,
            cache_read: None,
            cache_write: None,
        },
    };
    let pricing = ModelPricing {
        base: PricingRates {
            input: 1.0,
            ..Default::default()
        },
        tiers: vec![tier(500_000, 4.0), tier(200_000, 2.0)],
    };

    assert_eq!(pricing.rates_for(200_000).input, 1.0);
    assert_eq!(pricing.rates_for(200_001).input, 2.0);
    assert_eq!(pricing.rates_for(600_000).input, 4.0);

    let cost = pricing.cost(&Usage::simple(300_000, 0));
    assert_close(cost.input, 0.6);
}
