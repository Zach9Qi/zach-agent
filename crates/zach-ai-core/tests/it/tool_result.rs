//! `ToolResultOutput` 构造与厂商结果归一

use serde_json::json;
use zach_ai_core::ToolResultOutput;

#[test]
fn provider_result_string_maps_to_text_variants() {
    assert_eq!(
        ToolResultOutput::from_provider_result(false, json!("ok")),
        ToolResultOutput::text("ok")
    );
    assert_eq!(
        ToolResultOutput::from_provider_result(true, json!("boom")),
        ToolResultOutput::error_text("boom")
    );
}

#[test]
fn provider_result_non_string_maps_to_json_variants() {
    let value = json!({ "items": [1, 2] });
    assert_eq!(
        ToolResultOutput::from_provider_result(false, value.clone()),
        ToolResultOutput::json(value.clone())
    );
    assert_eq!(
        ToolResultOutput::from_provider_result(true, value.clone()),
        ToolResultOutput::ErrorJson {
            value,
            provider_options: None,
        }
    );
}
