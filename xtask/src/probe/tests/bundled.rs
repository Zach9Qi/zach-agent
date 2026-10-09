//! 内置场景及真实附件在编译时嵌入，防止文档示例与加载器格式漂移，并守住按协议复制的场景不互相走样。

use super::super::{config::Mode, scenario::Scenario, ProbeResult};
use serde_json::Value;
use std::path::Path;

const ALL_MODES: [Mode; 3] = [Mode::Generate, Mode::Stream, Mode::Agent];
const TOOL_SCENARIOS: [&str; 3] = [
    "responses-tools.json",
    "chat-tools.json",
    "anthropic-tools.json",
];

/// 内置场景源码及其支持的执行模式；新增场景文件时在此登记。
fn bundled() -> Vec<(&'static str, &'static str, &'static [Mode])> {
    vec![
        (
            "mixed.json",
            include_str!("../../../../scenarios/probe/mixed.json"),
            &ALL_MODES,
        ),
        (
            "responses-tools.json",
            include_str!("../../../../scenarios/probe/responses-tools.json"),
            &[Mode::Agent],
        ),
        (
            "chat-tools.json",
            include_str!("../../../../scenarios/probe/chat-tools.json"),
            &[Mode::Agent],
        ),
        (
            "anthropic-tools.json",
            include_str!("../../../../scenarios/probe/anthropic-tools.json"),
            &[Mode::Agent],
        ),
    ]
}

fn committed_file(path: &Path) -> &'static [u8] {
    match path.file_name().unwrap().to_str().unwrap() {
        "image-a.png" => include_bytes!("../../../../scenarios/probe/fixtures/image-a.png"),
        "image-b.png" => include_bytes!("../../../../scenarios/probe/fixtures/image-b.png"),
        "report-a.pdf" => include_bytes!("../../../../scenarios/probe/fixtures/report-a.pdf"),
        "report-b.pdf" => include_bytes!("../../../../scenarios/probe/fixtures/report-b.pdf"),
        "attachment-summary.json" => {
            include_bytes!("../../../../scenarios/probe/schemas/attachment-summary.json")
        }
        _ => panic!("内置场景引用了未知附件"),
    }
}

fn parse(source: &str, mode: Mode, allow_io: bool) -> ProbeResult<Scenario> {
    Scenario::parse(
        mode,
        source,
        Path::new("scenarios/probe/example.json"),
        |path| {
            assert!(allow_io, "不适用的模式应在读取文件前拒绝");
            Ok(committed_file(path).to_vec())
        },
    )
}

fn load(name: &str) -> Scenario {
    let (_, source, _) = bundled()
        .into_iter()
        .find(|(file, ..)| *file == name)
        .expect("未登记的内置场景");
    parse(source, Mode::Agent, true).unwrap()
}

fn expectations(scenario: &Scenario, keep: impl Fn(&Value) -> bool) -> Vec<Value> {
    scenario
        .expect
        .iter()
        .map(|expectation| serde_json::to_value(expectation).unwrap())
        .filter(keep)
        .collect()
}

#[test]
fn every_bundled_scenario_loads_with_its_committed_files() {
    for (name, source, supported) in bundled() {
        for mode in ALL_MODES {
            let result = parse(source, mode, supported.contains(&mode));
            assert_eq!(
                result.is_ok(),
                supported.contains(&mode),
                "{name} {mode:?}: {:?}",
                result.err()
            );
        }
    }
}

/// 三份工具场景按协议复制，只允许在请求选项与推理断言上有差异；任务输入和任务断言必须一致，
/// 防止改了一份提示词或预期数值而忘记同步另外两份。
#[test]
fn tool_scenarios_share_the_same_task_and_differ_only_in_protocol_details() {
    let is_task = |value: &Value| {
        !matches!(value["type"].as_str(), Some("reasoning" | "event"))
            && !(value["type"] == "replay" && value["content"] == "reasoning")
    };
    let reference = load(TOOL_SCENARIOS[0]);
    for name in &TOOL_SCENARIOS[1..] {
        let scenario = load(name);
        assert_eq!(scenario.request.prompt, reference.request.prompt, "{name}");
        assert_eq!(
            expectations(&scenario, is_task),
            expectations(&reference, is_task),
            "{name}"
        );
        assert_eq!(
            scenario.agent.is_some(),
            reference.agent.is_some(),
            "{name}"
        );
    }
}

/// 附件数值和校验码只写在断言里；四份场景共用同一组附件，答案必须一致。
#[test]
fn all_bundled_scenarios_agree_on_attachment_answers() {
    let is_json =
        |value: &Value| matches!(value["type"].as_str(), Some("json_equals" | "json_type"));
    let is_file = |part: &Value| part["type"] == "file";
    let reference = parse(bundled()[0].1, Mode::Stream, true).unwrap();
    let files = |scenario: &Scenario| {
        serde_json::to_value(&scenario.request.prompt).unwrap()["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .filter(|part| is_file(part))
            .cloned()
            .collect::<Vec<_>>()
    };
    for name in TOOL_SCENARIOS {
        let scenario = load(name);
        assert_eq!(files(&scenario), files(&reference), "{name}");
        assert_eq!(
            expectations(&scenario, is_json),
            expectations(&reference, is_json),
            "{name}"
        );
    }
}
