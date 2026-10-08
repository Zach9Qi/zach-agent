//! 文件随消息移动、助手与工具历史中的附件以及任意业务 JSON 的保真。

use super::{source, Mode, Scenario};
use serde_json::json;
use std::path::{Path, PathBuf};
use zach_ai_core::{CallOptions, FileData};

#[test]
fn reordering_messages_and_parts_keeps_each_file_with_its_own_path() {
    let mut value = source();
    let messages = value["request"]["prompt"]["messages"]
        .as_array_mut()
        .unwrap();
    messages[0]["content"].as_array_mut().unwrap().swap(1, 2);
    messages.insert(0, json!({"role":"system","content":"新增系统提示"}));
    let parsed = Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("cases/mixed.json"),
        |path| match path.to_str().unwrap().replace('\\', "/").as_str() {
            "cases/fixtures/report.pdf" => Ok(vec![2]),
            "cases/fixtures/image.png" => Ok(vec![1]),
            _ => panic!("未预期的附件"),
        },
    )
    .unwrap();
    let request = serde_json::to_value(parsed.request).unwrap();
    let content = &request["prompt"]["messages"][1]["content"];
    assert_eq!(content[1]["filename"], "说明.pdf");
    assert_eq!(content[1]["data"], json!({"type":"data","data":"Ag=="}));
    assert_eq!(content[2]["media_type"], "image/png");
    assert_eq!(content[2]["data"], json!({"type":"data","data":"AQ=="}));
    assert!(content[1].get("path").is_none());
    assert!(content[2].get("path").is_none());
}

#[test]
fn assistant_and_tool_history_load_files_including_repeated_paths() {
    let mut value = source();
    let file = json!({"type":"file","media_type":"image/png","filename":"图.png",
        "path":"fixtures/image.png","provider_options":{"openai":{"detail":"high"}}});
    let result = json!({"type":"tool_result","tool_call_id":"c1","tool_name":"render",
    "output":{"type":"content","value":[
        {"type":"text","text":"图片"}, file.clone()
    ]}});
    value["request"]["prompt"]["messages"] = json!([
        {"role":"assistant","content":[
            file,
            {"type":"reasoning_file","media_type":"image/png","path":"fixtures/image.png",
             "provider_options":{"openai":{"id":"reasoning_1"}}},
            result.clone()
        ]},
        {"role":"tool","content":[result]}
    ]);
    let mut paths = vec![];
    let parsed = Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("cases/history.json"),
        |path| {
            paths.push(path.to_owned());
            Ok(vec![42])
        },
    )
    .unwrap();
    assert_eq!(paths, vec![PathBuf::from("cases/fixtures/image.png"); 4]);
    let mut expected = value["request"].clone();
    for pointer in [
        "/prompt/messages/0/content/0",
        "/prompt/messages/0/content/1",
        "/prompt/messages/0/content/2/output/value/1",
        "/prompt/messages/1/content/0/output/value/1",
    ] {
        let block = expected.pointer_mut(pointer).unwrap();
        block.as_object_mut().unwrap().remove("path");
        block["data"] = serde_json::to_value(FileData::from_bytes(vec![42])).unwrap();
    }
    let expected: CallOptions = serde_json::from_value(expected).unwrap();
    assert_eq!(parsed.request, expected);
}

/// 形似附件的业务对象不能触发磁盘读取，也不能被改写成 Base64。
#[test]
fn business_paths_in_arbitrary_json_are_preserved_without_file_io() {
    let mut value = source();
    let business = json!({"type":"file","media_type":"image/png","path":"business-only.png"});
    let custom = json!({"type":"custom","kind":"example","data":business.clone()});
    value["request"]["provider_options"] = json!({"openai":{"payload":business.clone()}});
    value["request"]["prompt"]["messages"] = json!([
        {"role":"assistant","content":[
            {"type":"tool_call","tool_call_id":"c1","tool_name":"process","input":business.clone()},
            custom.clone()
        ]},
        {"role":"tool","content":[
            {"type":"tool_result","tool_call_id":"c1","tool_name":"process",
             "output":{"type":"json","value":business.clone()}},
            {"type":"tool_result","tool_call_id":"c2","tool_name":"process",
             "output":{"type":"error_json","value":business}},
            {"type":"tool_result","tool_call_id":"c3","tool_name":"process",
             "output":{"type":"content","value":[custom]}}
        ]}
    ]);
    let parsed = Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("x.json"),
        |_| panic!("业务 path 不能触发文件读取"),
    )
    .unwrap();
    let expected: CallOptions = serde_json::from_value(value["request"].clone()).unwrap();
    assert_eq!(parsed.request, expected);
}

#[test]
fn absolute_paths_are_not_rebased_to_the_scenario_directory() {
    let mut value = source();
    let root = if cfg!(windows) {
        "C:/fixtures"
    } else {
        "/fixtures"
    };
    let path = Path::new(root).join("image.png");
    value["request"]["prompt"]["messages"][0]["content"] = json!([
        {"type":"file","media_type":"image/png","path":path}
    ]);
    let mut paths = vec![];
    Scenario::parse(
        Mode::Stream,
        &value.to_string(),
        Path::new("cases/mixed.json"),
        |path| {
            paths.push(path.to_owned());
            Ok(vec![42])
        },
    )
    .unwrap();
    assert_eq!(paths, [path]);
}
