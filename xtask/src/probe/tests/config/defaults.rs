//! 位置参数、协议别名、默认场景与省略模式时的内置套件展开。

use super::super::super::config::{Config, Mode, Protocol};
use super::{generic, raw, single};
use crate::probe::args::Args;
use std::path::Path;

#[test]
fn positional_protocol_and_mode_accept_aliases_and_equal_their_flags() {
    for (positional, flags, protocol) in [
        (
            ["responses", "agent"],
            ["--protocol", "openai-responses", "--mode", "agent"],
            Protocol::OpenAiResponses,
        ),
        (
            ["chat", "generate"],
            ["--protocol", "openai-chat", "--mode", "generate"],
            Protocol::OpenAiChat,
        ),
        (
            ["anthropic", "stream"],
            ["--protocol", "anthropic-messages", "--mode", "stream"],
            Protocol::AnthropicMessages,
        ),
    ] {
        let short = single(&positional, generic);
        let long = single(&flags, generic);
        assert_eq!(short.protocol, protocol);
        assert_eq!(short.protocol, long.protocol);
        assert_eq!(short.mode, long.mode);
        assert_eq!(short.scenario_path, long.scenario_path);
    }
    for values in [
        vec!["responses", "--protocol", "chat"],
        vec!["responses", "stream", "--mode", "agent"],
        vec!["responses", "stream", "extra"],
        vec![""],
        vec![
            "responses",
            "stream",
            "--scenario",
            "a.json",
            "--scenario",
            "b.json",
        ],
    ] {
        assert!(
            Args::parse(&values.iter().map(|s| (*s).into()).collect::<Vec<_>>()).is_err(),
            "{values:?}"
        );
    }
}

#[test]
fn omitting_mode_without_scenario_runs_the_bundled_suite_in_a_fixed_order() {
    for (protocol, tools) in [
        ("responses", "responses-tools.json"),
        ("chat", "chat-tools.json"),
        ("anthropic", "anthropic-tools.json"),
    ] {
        let configs = Config::resolve(raw(&[protocol]), generic).unwrap();
        let runs: Vec<_> = configs
            .iter()
            .map(|c| {
                (
                    c.mode,
                    c.scenario_path.file_name().unwrap().to_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            runs,
            [
                (Mode::Generate, "mixed.json"),
                (Mode::Stream, "mixed.json"),
                (Mode::Agent, tools)
            ],
            "{protocol}"
        );
        for config in &configs {
            assert!(
                config.scenario_path.is_file(),
                "{}",
                config.scenario_path.display()
            );
            assert!(config.scenario_path.ends_with(
                Path::new("scenarios/probe").join(config.scenario_path.file_name().unwrap())
            ));
            assert_eq!(config.model, "generic-model");
            assert_eq!(config.api_key, "generic-key");
        }
    }
    let configs = Config::resolve(raw(&[]), generic).unwrap();
    assert_eq!(configs.len(), 3);
    assert!(configs
        .iter()
        .all(|c| c.protocol == Protocol::OpenAiResponses));
}

#[test]
fn a_single_mode_without_scenario_uses_that_modes_bundled_scenario() {
    for (mode, expected, file) in [
        ("generate", Mode::Generate, "mixed.json"),
        ("stream", Mode::Stream, "mixed.json"),
        ("agent", Mode::Agent, "anthropic-tools.json"),
    ] {
        let mut configs = Config::resolve(raw(&["anthropic", mode]), generic).unwrap();
        assert_eq!(configs.len(), 1);
        let config = configs.remove(0);
        assert_eq!(config.mode, expected);
        assert_eq!(config.scenario_path.file_name().unwrap(), file);
    }
}

/// 自定义场景的作者知道它适用的模式；省略时沿用最常用的 stream，而不是猜测或展开套件。
#[test]
fn a_custom_scenario_without_mode_runs_stream_once() {
    let mut configs = Config::resolve(raw(&["chat", "--scenario", "my.json"]), generic).unwrap();
    assert_eq!(configs.len(), 1);
    let config = configs.remove(0);
    assert_eq!(config.mode, Mode::Stream);
    assert_eq!(config.scenario_path, Path::new("my.json"));
    let config = single(&["chat", "agent", "--scenario", "my.json"], generic);
    assert_eq!(config.mode, Mode::Agent);
    assert_eq!(config.scenario_path, Path::new("my.json"));
}
