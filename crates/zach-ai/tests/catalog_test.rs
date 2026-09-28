use zach_ai::ModelCatalog;
use zach_ai_core::{ModelPricing, ModelProfile};

#[test]
fn builtin_catalog_parses_and_is_consistent() {
    let catalog = ModelCatalog::builtin();
    assert!(!catalog.is_empty());
    for profile in catalog.iter() {
        assert!(!profile.id.is_empty());
        assert!(profile.limits.context_window > 0, "{}", profile.id);
        assert!(profile.tool_call, "{} 应只收录支持工具调用的模型", profile.id);
    }
}

#[test]
fn builtin_catalog_covers_main_providers() {
    let catalog = ModelCatalog::builtin();
    for provider in ["anthropic", "openai", "deepseek"] {
        assert!(
            !catalog.provider_models(provider).is_empty(),
            "缺少厂商 {provider}"
        );
    }
}

#[test]
fn insert_overrides_existing_profile() {
    let mut catalog: ModelCatalog = [ModelProfile::new("local", "qwen3", 32_768, 8_192)]
        .into_iter()
        .collect();

    let mut custom = ModelProfile::new("local", "qwen3", 131_072, 8_192);
    custom.pricing = Some(ModelPricing::new(0.0, 0.0));
    assert!(catalog.insert(custom).is_some());

    let profile = catalog.get("local", "qwen3").unwrap();
    assert_eq!(profile.limits.context_window, 131_072);
    assert_eq!(catalog.len(), 1);
    assert!(catalog.get("local", "missing").is_none());
}

#[test]
fn from_json_reads_profile_array() {
    let catalog = ModelCatalog::from_json(
        r#"[{ "provider": "p", "id": "m", "name": "M",
              "limits": { "context_window": 1000, "max_output_tokens": 100 } }]"#,
    )
    .unwrap();
    assert_eq!(catalog.provider_models("p").len(), 1);
}
