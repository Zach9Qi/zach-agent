//! 槽位维护的内部不变量：封存语义、原地替换、未知 id 忽略与元数据合并
//!
//! 这些行为在公开 API 层只能间接观察，故直接针对私有槽位操作验证。

use super::*;
use serde_json::{json, Value};

fn meta(provider: &str, value: Value) -> Option<ProviderMetadata> {
    let mut metadata = ProviderMetadata::new();
    metadata.insert(provider, value);
    Some(metadata)
}

/// 读取工具槽位的（名称、入参、厂商执行、动态）四元组
fn tool(acc: &StreamAccumulator, id: &str) -> (String, String, bool, bool) {
    match &acc.content[acc.tool_index[id]] {
        OutputContent::ToolCall {
            tool_name,
            input,
            provider_executed,
            dynamic,
            ..
        } => (
            tool_name.clone(),
            input.clone(),
            *provider_executed,
            *dynamic,
        ),
        other => panic!("应为工具调用槽位: {other:?}"),
    }
}

fn tool_result(id: &str, result: Value, preliminary: bool) -> OutputContent {
    OutputContent::ToolResult {
        tool_call_id: id.into(),
        tool_name: "search".into(),
        result,
        is_error: false,
        preliminary,
        dynamic: false,
        provider_metadata: None,
    }
}

#[test]
fn merge_metadata_keeps_existing_when_incoming_is_none() {
    let mut slot = meta("openai", json!({ "a": 1 }));
    merge_metadata(&mut slot, None);
    assert_eq!(slot, meta("openai", json!({ "a": 1 })));
}

#[test]
fn merge_metadata_fills_empty_slot_then_merges_fields_per_provider() {
    let mut slot = None;
    merge_metadata(&mut slot, meta("openai", json!({ "a": 1 })));
    merge_metadata(&mut slot, meta("openai", json!({ "b": 2 })));
    assert_eq!(slot, meta("openai", json!({ "a": 1, "b": 2 })));
}

#[test]
fn ensure_text_reuses_slot_and_merges_metadata_for_same_id() {
    let mut acc = StreamAccumulator::new();
    let first = acc.ensure_text("t1", None);
    acc.append_text(first, "你", None);
    let again = acc.ensure_text("t1", meta("x", json!({ "k": 1 })));

    assert_eq!(first, again);
    assert_eq!(acc.content.len(), 1);
    assert!(matches!(
        &acc.content[first],
        OutputContent::Text { text, provider_metadata: Some(_) } if text == "你"
    ));
}

#[test]
fn finish_on_unknown_id_is_a_noop() {
    let mut acc = StreamAccumulator::new();
    acc.finish_text("nope", meta("x", json!({})));
    acc.finish_reasoning("nope", None);
    acc.finish_tool_input("nope", None);
    assert!(acc.content.is_empty());
}

#[test]
fn sealed_tool_ignores_later_deltas_and_flag_changes() {
    let mut acc = StreamAccumulator::new();
    acc.apply_tool_call(
        "c1".into(),
        "search".into(),
        r#"{"q":1}"#.into(),
        true,
        true,
        None,
    );
    assert!(acc.sealed_tools.contains("c1"));

    acc.append_tool_input("c1", "junk", None);
    acc.begin_tool("c1", "renamed".into(), false, false, None);
    assert_eq!(
        tool(&acc, "c1"),
        ("search".into(), r#"{"q":1}"#.into(), true, true)
    );
}

#[test]
fn complete_call_with_empty_input_does_not_seal() {
    let mut acc = StreamAccumulator::new();
    acc.apply_tool_call(
        "c1".into(),
        "search".into(),
        String::new(),
        false,
        false,
        None,
    );
    assert!(!acc.sealed_tools.contains("c1"));

    acc.append_tool_input("c1", r#"{"q":1}"#, None);
    assert_eq!(tool(&acc, "c1").1, r#"{"q":1}"#);
}

#[test]
fn begin_tool_after_early_delta_fills_name_without_new_slot() {
    let mut acc = StreamAccumulator::new();
    acc.append_tool_input("c1", "{", None);
    acc.begin_tool("c1", "search".into(), false, true, None);

    assert_eq!(acc.content.len(), 1);
    assert_eq!(tool(&acc, "c1"), ("search".into(), "{".into(), false, true));
}

#[test]
fn tool_result_for_same_call_replaces_in_place() {
    let mut acc = StreamAccumulator::new();
    acc.apply_tool_result(tool_result("c1", json!("50%"), true));
    acc.ensure_text("t", None);
    acc.apply_tool_result(tool_result("c1", json!("done"), false));
    acc.apply_tool_result(tool_result("c2", json!("other"), false));

    assert_eq!(acc.content.len(), 3, "c1 应原地替换而不是追加");
    assert!(matches!(
        &acc.content[0],
        OutputContent::ToolResult { tool_call_id, result, preliminary: false, .. }
            if tool_call_id == "c1" && result == "done"
    ));
    assert!(matches!(&acc.content[1], OutputContent::Text { .. }));
    assert!(matches!(
        &acc.content[2],
        OutputContent::ToolResult { tool_call_id, .. } if tool_call_id == "c2"
    ));
}

#[test]
fn non_result_content_is_ignored_by_apply_tool_result() {
    let mut acc = StreamAccumulator::new();
    acc.apply_tool_result(OutputContent::Text {
        text: "不是结果".into(),
        provider_metadata: None,
    });
    assert!(acc.content.is_empty());
}
