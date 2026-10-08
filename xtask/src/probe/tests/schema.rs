//! 共享 Schema 的相对路径、标准选项合并、配置冲突及跨模式模型入参。

use super::{
    super::{config::Mode, runner::run, scenario::Scenario},
    support::*,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use zach_ai_core::ResponseFormat;

fn source() -> Value {
    json!({
        "schema_file":"schemas/result.json",
        "request":{"prompt":{"messages":[{"role":"user","content":[{"type":"text","text":"计算"}]}]}},
        "expect":[{"type":"finish_reason","value":"stop"}]
    })
}

fn schema() -> Value {
    json!({"type":"object","properties":{"value":{"type":"integer"}},
        "required":["value"],"additionalProperties":false})
}

#[test]
fn file_reference_uses_scenario_directory_and_preserves_standard_format_metadata() {
    for format in [
        None,
        Some(json!({"type":"json","name":"amount","description":"金额"})),
    ] {
        let mut value = source();
        if let Some(format) = format.clone() {
            value["request"]["response_format"] = format;
        }
        let mut paths = vec![];
        let scenario = Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("cases/mixed.json"),
            |path| {
                paths.push(path.to_owned());
                // 与场景文件一致，允许 Windows 编辑器生成的 UTF-8 BOM。
                Ok(format!("\u{feff}{}", schema()).into_bytes())
            },
        )
        .unwrap();
        assert_eq!(paths, [PathBuf::from("cases/schemas/result.json")]);
        assert_eq!(
            scenario.request.response_format,
            Some(ResponseFormat::Json {
                schema: Some(schema()),
                name: format.as_ref().map(|_| "amount".into()),
                description: format.as_ref().map(|_| "金额".into())
            })
        );
    }
}

#[test]
fn inline_schema_and_json_mode_remain_available_without_reading_files() {
    for format in [
        json!({"type":"json"}),
        json!({"type":"json","schema":schema()}),
    ] {
        let mut value = source();
        value.as_object_mut().unwrap().remove("schema_file");
        value["request"]["response_format"] = format.clone();
        let scenario = Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(scenario.request.response_format.unwrap()).unwrap(),
            format
        );
    }
}

#[test]
fn conflicting_formats_and_invalid_scenarios_fail_before_loading_schema_or_attachments() {
    for format in [
        json!({"type":"text"}),
        json!({"type":"json","schema":schema()}),
        json!({"type":"json","schema":null}),
        Value::Null,
    ] {
        let mut value = source();
        value["request"]["response_format"] = format;
        assert!(Scenario::parse(
            Mode::Stream,
            &value.to_string(),
            Path::new("x.json"),
            |_| panic!("冲突配置不能读取文件")
        )
        .is_err());
    }
    let mut value = source();
    value["schema_file"] = json!("");
    assert!(Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!()
    )
    .is_err());
    value = source();
    value["timeout_secs"] = json!(0);
    assert!(Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!()
    )
    .is_err());
    value = source();
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
fn unreadable_invalid_or_non_object_schema_files_fail_without_exposing_contents() {
    let source = source().to_string();
    for contents in [
        vec![],
        vec![0xff],
        b"[]".to_vec(),
        b"true".to_vec(),
        b"null".to_vec(),
        br#"{"type": "private-secret" invalid}"#.to_vec(),
    ] {
        let error = Scenario::parse(Mode::Stream, &source, Path::new("x.json"), |_| {
            Ok(contents.clone())
        })
        .err()
        .unwrap();
        assert!(!error.contains("private-secret"));
    }
    let error = Scenario::parse(Mode::Stream, &source, Path::new("x.json"), |_| {
        Err("找不到文件".into())
    })
    .err()
    .unwrap();
    assert!(error.contains("读取 Schema 文件"));
    assert!(error.contains("找不到文件"));
}

#[test]
fn internal_schema_references_are_preserved_without_fetching_other_files() {
    let expected = json!({"type":"object","properties":{"value":{"$ref":"#/$defs/Number"}},
        "required":["value"],"additionalProperties":false,"$defs":{"Number":{"type":"integer"}}});
    let mut reads = 0;
    let scenario = Scenario::parse(
        Mode::Stream,
        &source().to_string(),
        Path::new("x.json"),
        |_| {
            reads += 1;
            Ok(expected.to_string().into_bytes())
        },
    )
    .unwrap();
    assert_eq!(reads, 1);
    assert_eq!(
        scenario.request.response_format,
        Some(ResponseFormat::json_schema(expected))
    );
}

#[tokio::test]
async fn referenced_schema_reaches_generate_stream_and_every_agent_turn() {
    for mode in ["generate", "stream", "agent"] {
        let mut value = source();
        let config = config(&["--mode", mode]);
        let scripts = if mode == "agent" {
            value["agent"] = json!({"tools":["add"],"next_tool_choice":{"type":"none"}});
            value["request"]["tool_choice"] = json!({"type":"tool","tool_name":"add"});
            vec![
                Script::Reply(tool_reply("c1")),
                Script::Reply(text_reply(r#"{"value":42}"#)),
            ]
        } else {
            vec![Script::Reply(text_reply(r#"{"value":42}"#))]
        };
        let scenario =
            Scenario::parse(config.mode, &value.to_string(), Path::new("x.json"), |_| {
                Ok(schema().to_string().into_bytes())
            })
            .unwrap();
        let model = Model::new(scripts);
        let (reporter, _) = output();
        run(&config, &scenario, model.clone(), reporter)
            .await
            .unwrap();
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), if mode == "agent" { 2 } else { 1 });
        for request in requests.iter() {
            assert_eq!(
                request.response_format,
                Some(ResponseFormat::json_schema(schema()))
            );
        }
    }
}
