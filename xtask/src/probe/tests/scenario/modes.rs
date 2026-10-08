//! 同一场景跨执行模式复用，并在读取附件前拒绝模式冲突和旧的 mode 字段。

use super::{source, Mode, Scenario};
use serde_json::json;
use std::path::Path;

#[test]
fn the_same_scenario_loads_identical_requests_in_all_modes() {
    let source = source().to_string();
    let mut requests = vec![];
    for mode in [Mode::Generate, Mode::Stream, Mode::Agent] {
        let parsed =
            Scenario::parse(mode, &source, Path::new("cases/x.json"), |_| Ok(vec![42])).unwrap();
        assert!(parsed.agent.is_none());
        assert_eq!(
            parsed.max_steps(mode),
            if mode == Mode::Agent { 4 } else { 1 }
        );
        requests.push(parsed.request);
    }
    assert!(requests.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn scenario_mode_is_rejected_instead_of_overriding_the_cli() {
    for mode in [Mode::Generate, Mode::Stream, Mode::Agent] {
        let mut value = source();
        value["mode"] = json!("stream");
        assert!(
            Scenario::parse(mode, &value.to_string(), Path::new("x.json"), |_| {
                panic!("旧字段应在文件读取前拒绝")
            })
            .is_err()
        );
    }
}

#[test]
fn incompatible_settings_and_assertions_fail_before_reading_files() {
    for (mode, field, value) in [
        (Mode::Generate, "agent", json!({"tools":["add"]})),
        (Mode::Stream, "agent", json!({"tools":["add"]})),
        (
            Mode::Generate,
            "expect",
            json!([{"type":"event","event":"text_delta","min":1}]),
        ),
        (
            Mode::Stream,
            "expect",
            json!([{"type":"replay","content":"input"}]),
        ),
        (
            Mode::Generate,
            "expect",
            json!([{"type":"tool_result","name":"add","value":{"sum":42}}]),
        ),
    ] {
        let mut source = source();
        source[field] = value;
        assert!(
            Scenario::parse(mode, &source.to_string(), Path::new("x.json"), |_| {
                panic!("不适用的设置不能静默忽略")
            })
            .is_err()
        );
    }
}

#[test]
fn default_agent_settings_do_not_silently_replace_standard_tools() {
    for (field, value) in [
        ("tools", json!([])),
        ("tool_choice", json!({"type":"required"})),
        ("tool_choice", json!({"type":"tool","tool_name":"add"})),
    ] {
        let mut source = source();
        source["request"][field] = value;
        assert!(Scenario::parse(
            Mode::Agent,
            &source.to_string(),
            Path::new("x.json"),
            |_| { panic!("Agent 默认没有注册工具") }
        )
        .is_err());
    }
}
