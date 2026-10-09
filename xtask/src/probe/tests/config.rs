//! 命令行解析、配置优先级和凭据来源隔离。

use super::super::{
    args::Args,
    config::{Config, Mode, Protocol},
};
use super::support::args;

#[test]
fn help_and_chat_protocol_are_accepted_before_credentials_validation() {
    assert!(Args::parse(&["--help".into()]).unwrap().is_none());
    let result = Config::resolve(
        args(&["--protocol", "openai-chat", "--model", "chat-model"]),
        |name| (name == "PROBE_API_KEY").then(|| "secret".into()),
    )
    .unwrap();
    assert_eq!(result.protocol, Protocol::OpenAiChat);
}

#[test]
fn command_line_overrides_probe_environment_settings() {
    let env = |key: &str| match key {
        "PROBE_MODEL" => Some("generic-model".into()),
        "PROBE_BASE_URL" => Some("https://generic.example/v1/".into()),
        "PROBE_API_KEY" => Some("generic-key".into()),
        _ => None,
    };
    let selected = Config::resolve(args(&[]), env).unwrap();
    assert_eq!(selected.protocol, Protocol::OpenAiResponses);
    assert_eq!(selected.scenario_path, std::path::Path::new("test.json"));
    assert_eq!(selected.model, "generic-model");
    assert_eq!(selected.base_url, "https://generic.example/v1");
    assert_eq!(selected.api_key, "generic-key");
    let selected = Config::resolve(
        args(&["--model=cli-model", "--base-url", "https://cli.example/v2"]),
        env,
    )
    .unwrap();
    assert_eq!(selected.model, "cli-model");
    assert_eq!(selected.base_url, "https://cli.example/v2");
}

#[test]
fn explicit_key_variable_does_not_fall_back_to_another_account() {
    let selected = Config::resolve(
        args(&["--model", "m", "--api-key-env", "TEAM_KEY"]),
        |key| match key {
            "TEAM_KEY" => Some("team-secret".into()),
            "PROBE_API_KEY" => Some("other-secret".into()),
            _ => None,
        },
    )
    .unwrap();
    assert_eq!(selected.api_key, "team-secret");
    let missing = Config::resolve(args(&["--model", "m", "--api-key-env", "MISSING"]), |key| {
        (key == "PROBE_API_KEY").then(|| "other-secret".into())
    });
    assert!(missing.is_err());
}

#[test]
fn provider_environment_variables_are_never_used_as_implicit_fallbacks() {
    let legacy = |key: &str| match key {
        "OPENAI_MODEL" => Some("provider-model".into()),
        "OPENAI_BASE_URL" => Some("https://legacy.example/v1".into()),
        "OPENAI_API_KEY" => Some("provider-key".into()),
        _ => None,
    };
    assert!(Config::resolve(args(&[]), legacy)
        .err()
        .unwrap()
        .contains("PROBE_MODEL"));
    assert!(Config::resolve(args(&["--model", "m"]), legacy)
        .err()
        .unwrap()
        .contains("PROBE_API_KEY"));
    let selected = Config::resolve(args(&["--model", "m"]), |key| {
        assert!(!key.starts_with("OPENAI_"), "不能隐式读取旧变量");
        (key == "PROBE_API_KEY").then(|| "probe-key".into())
    })
    .unwrap();
    assert_eq!(selected.base_url, "https://api.openai.com/v1");
    let explicit = Config::resolve(
        args(&["--model", "m", "--api-key-env", "OPENAI_API_KEY"]),
        legacy,
    )
    .unwrap();
    assert_eq!(explicit.api_key, "provider-key");
}

#[test]
fn execution_mode_is_selected_only_by_cli_and_defaults_to_stream() {
    assert_eq!(super::support::config(&[]).mode, Mode::Stream);
    for (name, expected) in [
        ("generate", Mode::Generate),
        ("stream", Mode::Stream),
        ("agent", Mode::Agent),
    ] {
        assert_eq!(super::support::config(&["--mode", name]).mode, expected);
    }
    let invalid = Config::resolve(args(&["--mode", "unknown"]), |_| {
        panic!("非法模式不能读取凭据")
    });
    assert!(invalid.err().unwrap().contains("--mode"));
}

#[test]
fn invalid_flags_ranges_and_sensitive_urls_fail_before_running() {
    for values in [
        vec!["--raw=true"],
        vec!["--model"],
        vec!["--model", "--raw"],
        vec!["--scenario", "a.json", "--scenario", "b.json"],
        vec!["--api-key=secret"],
        vec!["--mode"],
        vec!["--mode", "stream", "--mode", "generate"],
        vec!["--prompt", "你好"],
        vec!["--max-output-tokens", "100"],
        vec!["--timeout-secs", "10"],
        vec!["--max-steps", "2"],
    ] {
        assert!(Args::parse(&values.iter().map(|s| (*s).into()).collect::<Vec<_>>()).is_err());
    }
    for values in [
        vec!["--base-url", "https://user:secret@example.com/v1"],
        vec!["--base-url", "https://example.com/v1?key=secret"],
        vec!["--base-url", "https://example.com/v1/responses"],
    ] {
        assert!(Config::resolve(args(&values), |_| Some("placeholder".into())).is_err());
    }
    assert!(Config::resolve(args(&[]), |_| None).is_err());
}

#[test]
fn missing_scenario_fails_before_reading_credentials() {
    let args = Args::parse(&[]).unwrap().unwrap();
    let result = Config::resolve(args, |_| panic!("缺少场景时不应读取凭据"));
    assert!(matches!(result, Err(error) if error.contains("--scenario")));
}
