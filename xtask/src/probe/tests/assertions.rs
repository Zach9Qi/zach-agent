//! 内容、推理与事件断言的成功和失败边界，防止宽松条件误报成功。

use super::{super::runner::run, support::*};
use serde_json::{json, Value};
use zach_ai_core::{StreamPart, Usage};

async fn check(mode: &str, expect: Value, parts: Vec<StreamPart>) -> (bool, Buffer) {
    let scenario = parse_scenario(
        mode,
        json!({
            "request":{"prompt":{"messages":[{"role":"user","content":[{"type":"text","text":"问题"}]}]}},
            "expect":expect
        }),
    );
    let (reporter, buffer) = output();
    let success = run(
        &config(&["--mode", mode]),
        &scenario,
        Model::new(vec![Script::Reply(parts)]),
        reporter,
    )
    .await
    .is_ok();
    (success, buffer)
}

#[tokio::test]
async fn json_assertions_check_types_values_and_missing_null_fields() {
    for mode in ["generate", "stream"] {
        let expect = json!([
            {"type":"json_equals","pointer":"/total","value":910},
            {"type":"json_type","pointer":"","kind":"object"},
            {"type":"json_type","pointer":"/items","kind":"array"},
            {"type":"json_equals","pointer":"/optional","value":null}
        ]);
        assert!(
            check(
                mode,
                expect.clone(),
                text_reply(r#"{"total":910,"items":[],"optional":null}"#)
            )
            .await
            .0
        );
        for text in [
            r#"{"total":911,"items":[],"optional":null}"#,
            r#"{"total":"910","items":[],"optional":null}"#,
            r#"{"total":910,"items":[]}"#,
            "```json\n{}\n```",
        ] {
            assert!(!check(mode, expect.clone(), text_reply(text)).await.0);
        }
    }
}

#[tokio::test]
async fn reasoning_tokens_metadata_and_visible_summary_are_distinct_evidence() {
    let mut tokens = text_reply("答案");
    if let StreamPart::Finish { usage, .. } = &mut tokens[1] {
        *usage = Usage::simple(10, 20);
        usage.output_tokens.reasoning = Some(8);
    }
    let mut metadata = text_reply("答案");
    metadata.insert(
        0,
        StreamPart::ReasoningStart {
            id: "r".into(),
            provider_metadata: None,
        },
    );
    metadata.insert(1, StreamPart::ReasoningEnd { id:"r".into(), provider_metadata:Some(
        serde_json::from_value(json!({"openai":{"responses_item":{"type":"reasoning","encrypted_content":"opaque"}}})).unwrap()
    ) });
    let mut summary = text_reply("答案");
    summary.insert(
        0,
        StreamPart::ReasoningDelta {
            id: "r".into(),
            delta: "摘要".into(),
            provider_metadata: None,
        },
    );
    for mode in ["generate", "stream"] {
        for (parts, evidence) in [
            (tokens.clone(), "tokens"),
            (metadata.clone(), "metadata"),
            (summary.clone(), "summary"),
        ] {
            for expected in ["any", evidence] {
                assert!(
                    check(
                        mode,
                        json!([{"type":"reasoning","evidence":expected}]),
                        parts.clone()
                    )
                    .await
                    .0
                );
            }
            if evidence != "summary" {
                assert!(
                    !check(
                        mode,
                        json!([{"type":"reasoning","evidence":"summary"}]),
                        parts
                    )
                    .await
                    .0
                );
            }
        }
        assert!(
            !check(
                mode,
                json!([{"type":"reasoning","evidence":"any"}]),
                text_reply("答案")
            )
            .await
            .0
        );
    }
}

#[tokio::test]
async fn all_assertions_report_results_and_one_failure_fails_the_probe() {
    let (success, buffer) = check(
        "stream",
        json!([
            {"type":"text_contains","value":"答案"},
            {"type":"text_equals","value":"别的内容"},
            {"type":"event","event":"text_delta","min":1},
            {"type":"event","event":"reasoning_delta","min":1}
        ]),
        text_reply("答案"),
    )
    .await;
    assert!(!success);
    let results: Vec<_> = buffer
        .lines()
        .into_iter()
        .filter(|v| v["type"] == "assertion_result")
        .map(|v| v["data"]["success"].clone())
        .collect();
    assert_eq!(
        results,
        [json!(true), json!(false), json!(true), json!(false)]
    );
    assert_eq!(buffer.lines().last().unwrap()["data"]["success"], false);
}

#[tokio::test]
async fn tool_only_responses_can_be_checked_without_requiring_text() {
    let expect = json!([
        {"type":"finish_reason","value":"tool_calls"},
        {"type":"tool_call","name":"add","input":{"a":40,"b":2}}
    ]);
    assert!(check("generate", expect, tool_reply("c1")).await.0);
    assert!(
        !check(
            "stream",
            json!([{"type":"tool_call","name":"add","input":{"a":1,"b":2}}]),
            tool_reply("c1")
        )
        .await
        .0
    );
}

#[tokio::test]
async fn protocol_errors_cannot_pass_even_when_expected_text_is_present() {
    let mut parts = vec![StreamPart::Error {
        message: "错误".into(),
        raw: None,
    }];
    parts.extend(text_reply("答案"));
    assert!(
        !check(
            "stream",
            json!([{"type":"text_contains","value":"答案"}]),
            parts
        )
        .await
        .0
    );
}
