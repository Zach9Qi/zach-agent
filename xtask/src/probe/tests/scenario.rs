//! 标准请求保真、消息内本地文件路径以及配置错误在网络请求前的拒绝。

mod history;
mod modes;

use super::super::{config::Mode, scenario::Scenario};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use zach_ai_core::{CallOptions, FileData};

fn source() -> Value {
    json!({"request":{
        "prompt":{"messages":[{"role":"user","content":[
            {"type":"text","text":"结合附件回答"},
            {"type":"file","media_type":"image/png","path":"fixtures/image.png",
             "provider_options":{"openai":{"detail":"high"}}},
            {"type":"file","media_type":"application/pdf","filename":"说明.pdf","path":"fixtures/report.pdf"},
            {"type":"text","text":"分别列出来源"}
        ]}]},
        "reasoning":"high", "include_raw_chunks":true, "max_output_tokens":2048,
        "provider_options":{"openai":{"reasoning":{"summary":"auto"}}}
    }, "expect":[{"type":"text_nonempty"}]})
}

#[test]
fn inline_paths_preserve_standard_options_message_order_and_filenames() {
    let source = source();
    let mut paths = vec![];
    let scenario = Scenario::parse(
        Mode::Stream,
        &source.to_string(),
        Path::new("cases/mixed.json"),
        |path| {
            paths.push(path.to_owned());
            Ok(vec![paths.len() as u8])
        },
    )
    .unwrap();
    assert_eq!(
        paths,
        [
            PathBuf::from("cases/fixtures/image.png"),
            PathBuf::from("cases/fixtures/report.pdf")
        ]
    );
    let mut expected = source["request"].clone();
    for index in 1..=2 {
        let file = &mut expected["prompt"]["messages"][0]["content"][index];
        file.as_object_mut().unwrap().remove("path");
        file["data"] = serde_json::to_value(FileData::from_bytes(vec![index as u8])).unwrap();
    }
    let expected: CallOptions = serde_json::from_value(expected).unwrap();
    assert_eq!(scenario.request, expected);
}

#[test]
fn invalid_paths_fail_before_reading_any_file() {
    for path in [
        json!(""),
        json!(" \t "),
        json!(42),
        json!(false),
        json!({}),
        json!([]),
        Value::Null,
    ] {
        let mut value = source();
        value["request"]["prompt"]["messages"][0]["content"][2]["path"] = path;
        let error = Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!("所有路径应在读取文件前校验"),
        )
        .err()
        .unwrap();
        assert!(error.contains("path 必须是非空字符串"));
    }
}

#[test]
fn explicit_data_conflicts_with_path_even_when_empty_or_null() {
    for data in [
        json!({"type":"data","data":""}),
        json!({"type":"data","data":"AQ=="}),
        json!({"type":"url","url":"https://example.com/a.png"}),
        json!({"type":"reference","reference":{"openai":"file_1"}}),
        json!({"type":"text","text":"内容"}),
        Value::Null,
    ] {
        let mut value = source();
        // 第二个附件冲突时，第一个有效附件也不能提前读取。
        value["request"]["prompt"]["messages"][0]["content"][2]["data"] = data;
        let error = Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!("不能覆盖显式 data"),
        )
        .err()
        .unwrap();
        assert!(error.contains("不能同时提供 data 和 path"));
    }
}

#[test]
fn missing_data_and_path_fail_before_reading_any_file() {
    for index in 1..=2 {
        let mut value = source();
        value["request"]["prompt"]["messages"][0]["content"][index]
            .as_object_mut()
            .unwrap()
            .remove("path");
        let error = Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!("缺少附件来源时不应读取文件"),
        )
        .err()
        .unwrap();
        assert!(error.contains("data 或 path"));
    }
}

#[test]
fn missing_or_empty_files_fail_instead_of_sending_an_empty_attachment() {
    let source = source().to_string();
    for result in [Err("找不到文件".into()), Ok(vec![])] {
        assert!(
            Scenario::parse(Mode::Stream, &source, Path::new("x.json"), |_| result
                .clone())
            .is_err()
        );
    }
}

#[test]
fn native_urls_references_and_inline_data_need_no_file_loader() {
    let mut value = source();
    value["request"]["prompt"]["messages"][0]["content"] = json!([
        {"type":"file","media_type":"image/png","data":{"type":"url","url":"https://example.com/a.png"}},
        {"type":"file","media_type":"application/pdf","data":{"type":"reference","reference":{"openai":"file_1"}}},
        {"type":"file","media_type":"image/png","data":{"type":"data","data":"AQID"}},
        {"type":"file","media_type":"text/plain","data":{"type":"text","text":"内联内容"}}
    ]);
    let scenario = Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!(),
    )
    .unwrap();
    let expected: CallOptions = serde_json::from_value(value["request"].clone()).unwrap();
    assert_eq!(scenario.request, expected);
}

#[test]
fn obsolete_assets_and_misplaced_paths_are_rejected_without_io() {
    for pointer in ["/assets", "/request/prompt/messages/0/content/0/path"] {
        let mut value = source();
        if pointer == "/assets" {
            value["assets"] = json!([]);
        } else {
            value["request"]["prompt"]["messages"][0]["content"][0]["path"] = json!("text.txt");
        }
        assert!(Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!()
        )
        .is_err());
    }
}

#[test]
fn local_files_require_a_nonempty_media_type_before_io() {
    for media_type in [json!(""), json!("  "), json!(42), Value::Null] {
        let mut value = source();
        value["request"]["prompt"]["messages"][0]["content"][2]["media_type"] = media_type;
        assert!(Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!()
        )
        .is_err());
    }
}

#[test]
fn invalid_configuration_and_assertions_are_rejected_without_io() {
    for (pointer, replacement) in [
        ("/timeout_secs", json!(0)),
        ("/timeout_secs", json!(3601)),
        ("/request/max_output_tokens", json!(0)),
        ("/request/prompt/messages", json!([])),
        ("/expect", json!([])),
        ("/expect", json!([{"type":"text_contains","value":""}])),
        (
            "/expect",
            json!([{"type":"json_equals","pointer":"/~2","value":1}]),
        ),
        ("/expect", json!([{"type":"steps","min":4,"max":1}])),
        (
            "/expect",
            json!([{"type":"event","event":"reasoning_delta","min":0}]),
        ),
        (
            "/expect",
            json!([{"type":"text_equals","value":"答案","requires":"reasoning_summary"}]),
        ),
        (
            "/expect",
            json!([{"type":"event","event":"text_delta","min":1,"requires":"reasoning_stream"}]),
        ),
        (
            "/expect",
            json!([{"type":"replay","content":"input","requires":"reasoning_replay"}]),
        ),
        ("/expect", json!([{"type":"replay","content":"input"}])),
        ("/expect", json!([{"type":"text_nonempty","typo":true}])),
    ] {
        let mut value = source();
        if pointer == "/timeout_secs" {
            value["timeout_secs"] = replacement;
        } else {
            *value.pointer_mut(pointer).unwrap() = replacement;
        }
        assert!(
            Scenario::parse(
                Mode::Stream,
                &value.to_string(),
                Path::new("x.json"),
                |_| panic!("配置应先校验")
            )
            .is_err(),
            "{pointer}"
        );
    }
    let mut value = source();
    value["request"]["reasonning"] = json!("high");
    assert!(Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!()
    )
    .is_err());
}

#[test]
fn agent_settings_cannot_silently_override_standard_tool_declarations() {
    for settings in [
        json!({"tools":["unknown"]}),
        json!({"tools":["add","add"]}),
        json!({"max_steps":0}),
        json!({"tools":[],"next_tool_choice":{"type":"required"}}),
    ] {
        let mut value = source();
        value["agent"] = settings;
        assert!(Scenario::parse(
            Mode::Agent,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!()
        )
        .is_err());
    }
    let mut value = source();
    value["agent"] = json!({"tools":["add"]});
    value["request"]["tools"] = json!([]);
    assert!(Scenario::parse(
        Mode::Agent,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!()
    )
    .is_err());
}

#[test]
fn malformed_json_and_trailing_data_do_not_echo_private_values() {
    let error = Scenario::parse(
        Mode::Stream,
        r#"{"mode":"private-secret"}"#,
        Path::new("x.json"),
        |_| panic!(),
    )
    .err()
    .unwrap();
    assert!(!error.contains("private-secret"));
    assert!(Scenario::parse(
        Mode::Stream,
        &(source().to_string() + " true"),
        Path::new("x.json"),
        |_| panic!()
    )
    .is_err());
}
