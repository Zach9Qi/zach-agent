//! 自动生成的输出约束与已入库 Schema 保持一致，防止修改 Rust 类型后漏更新。

use super::schemas;
use serde_json::Value;

#[test]
fn checked_in_schemas_match_the_rust_output_types() {
    for (name, generated) in schemas() {
        let source = match name {
            "attachment-summary.json" => {
                include_str!("../../../scenarios/probe/schemas/attachment-summary.json")
            }
            _ => panic!("新增类型后请将对应的共享 Schema 加入校验"),
        };
        let committed: Value = serde_json::from_str(source).unwrap();
        assert_eq!(
            generated, committed,
            "{name} 已过期，请运行 cargo xtask probe-schemas"
        );
    }
}

#[test]
fn result_contracts_require_all_fields_and_disallow_extra_properties() {
    for (name, schema) in schemas() {
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let properties = schema["properties"].as_object().unwrap();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(properties.len(), required.len(), "{name}");
        for (field, constraint) in properties {
            assert!(required.iter().any(|v| v == field), "{name}.{field}");
            assert!(matches!(
                constraint["type"].as_str(),
                Some("integer" | "string")
            ));
            // 输出形状可以发送给模型，测试答案只留在 expect 中。
            for keyword in ["const", "enum", "default", "examples"] {
                assert!(
                    constraint.get(keyword).is_none(),
                    "{name}.{field}.{keyword}"
                );
            }
        }
    }
}
