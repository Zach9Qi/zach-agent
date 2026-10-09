//! 命令行解析、厂商分组的配置优先级和凭据来源隔离；位置参数与默认场景见 defaults。

mod defaults;

use super::super::{
    args::Args,
    config::{Config, Mode, Protocol},
};
use super::support::args;

/// 不自动补 `--scenario` 的原始参数，用于验证默认场景与套件展开。
pub(super) fn raw(values: &[&str]) -> Args {
    Args::parse(&values.iter().map(|s| (*s).into()).collect::<Vec<_>>())
        .unwrap()
        .unwrap()
}

pub(super) fn generic(name: &str) -> Option<String> {
    match name {
        "PROBE_MODEL" => Some("generic-model".into()),
        "PROBE_API_KEY" => Some("generic-key".into()),
        _ => None,
    }
}

pub(super) fn single(values: &[&str], env: impl Fn(&str) -> Option<String>) -> Config {
    let mut configs = Config::resolve(args(values), env).unwrap();
    assert_eq!(configs.len(), 1);
    configs.remove(0)
}

#[test]
fn help_and_chat_protocol_are_accepted_before_credentials_validation() {
    assert!(Args::parse(&["--help".into()]).unwrap().is_none());
    let result = single(
        &["--protocol", "openai-chat", "--model", "chat-model"],
        |name| (name == "PROBE_API_KEY").then(|| "secret".into()),
    );
    assert_eq!(result.protocol, Protocol::OpenAiChat);
    let result = single(
        &["--protocol", "anthropic-messages", "--model", "claude"],
        |name| (name == "PROBE_API_KEY").then(|| "secret".into()),
    );
    assert_eq!(result.protocol, Protocol::AnthropicMessages);
    assert_eq!(result.base_url, "https://api.anthropic.com");
    assert!(Config::resolve(
        args(&["--protocol", "unknown", "--model", "m"]),
        |_| panic!("未知协议不能读取凭据")
    )
    .is_err());
}

#[test]
fn vendor_group_variables_override_generic_ones_only_for_their_own_protocols() {
    let env = |key: &str| match key {
        "PROBE_MODEL" => Some("generic-model".into()),
        "PROBE_BASE_URL" => Some("https://generic.example/v1".into()),
        "PROBE_API_KEY" => Some("generic-key".into()),
        "PROBE_ANTHROPIC_MODEL" => Some("claude".into()),
        "PROBE_ANTHROPIC_BASE_URL" => Some("https://anthropic.example".into()),
        "PROBE_ANTHROPIC_API_KEY" => Some("anthropic-key".into()),
        "PROBE_OPENAI_MODEL" => Some("gpt".into()),
        _ => None,
    };
    let anthropic = single(&["anthropic"], env);
    assert_eq!(anthropic.model, "claude");
    assert_eq!(anthropic.base_url, "https://anthropic.example");
    assert_eq!(anthropic.api_key, "anthropic-key");
    for protocol in ["responses", "chat"] {
        let openai = single(&[protocol], env);
        assert_eq!(openai.model, "gpt");
        assert_eq!(openai.base_url, "https://generic.example/v1");
        assert_eq!(openai.api_key, "generic-key");
    }
    let cli = single(&["anthropic", "--model", "cli-model"], env);
    assert_eq!(cli.model, "cli-model");
    // 分组变量显式留空视为缺失，不回退到通用组的另一套凭据。
    let blank = Config::resolve(args(&["anthropic"]), |key| match key {
        "PROBE_ANTHROPIC_API_KEY" => Some(String::new()),
        other => env(other),
    });
    assert!(blank.is_err());
}

#[test]
fn command_line_overrides_probe_environment_settings() {
    let env = |key: &str| match key {
        "PROBE_MODEL" => Some("generic-model".into()),
        "PROBE_BASE_URL" => Some("https://generic.example/v1/".into()),
        "PROBE_API_KEY" => Some("generic-key".into()),
        _ => None,
    };
    let selected = single(&[], env);
    assert_eq!(selected.protocol, Protocol::OpenAiResponses);
    assert_eq!(selected.scenario_path, std::path::Path::new("test.json"));
    assert_eq!(selected.model, "generic-model");
    assert_eq!(selected.base_url, "https://generic.example/v1");
    assert_eq!(selected.api_key, "generic-key");
    let selected = single(
        &["--model=cli-model", "--base-url", "https://cli.example/v2"],
        env,
    );
    assert_eq!(selected.model, "cli-model");
    assert_eq!(selected.base_url, "https://cli.example/v2");
}

#[test]
fn explicit_key_variable_does_not_fall_back_to_another_account() {
    let selected = single(
        &["--model", "m", "--api-key-env", "TEAM_KEY"],
        |key| match key {
            "TEAM_KEY" => Some("team-secret".into()),
            "PROBE_OPENAI_API_KEY" => Some("group-secret".into()),
            "PROBE_API_KEY" => Some("other-secret".into()),
            _ => None,
        },
    );
    assert_eq!(selected.api_key, "team-secret");
    let missing = Config::resolve(args(&["--model", "m", "--api-key-env", "MISSING"]), |key| {
        matches!(key, "PROBE_API_KEY" | "PROBE_OPENAI_API_KEY").then(|| "other-secret".into())
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
    let selected = single(&["--model", "m"], |key| {
        assert!(!key.starts_with("OPENAI_"), "不能隐式读取旧变量");
        (key == "PROBE_API_KEY").then(|| "probe-key".into())
    });
    assert_eq!(selected.base_url, "https://api.openai.com/v1");
    let explicit = single(&["--model", "m", "--api-key-env", "OPENAI_API_KEY"], legacy);
    assert_eq!(explicit.api_key, "provider-key");
}

#[test]
fn execution_mode_is_selected_only_by_cli() {
    for (name, expected) in [
        ("generate", Mode::Generate),
        ("stream", Mode::Stream),
        ("agent", Mode::Agent),
    ] {
        assert_eq!(super::support::config(&["--mode", name]).mode, expected);
        assert_eq!(super::support::config(&["responses", name]).mode, expected);
    }
    let invalid = Config::resolve(args(&["--mode", "unknown"]), |_| {
        panic!("非法模式不能读取凭据")
    });
    assert!(invalid.err().unwrap().contains("--mode"));
    let invalid = Config::resolve(raw(&["responses", "unknown"]), |_| {
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
    assert!(Config::resolve(
        args(&[
            "--protocol",
            "anthropic-messages",
            "--base-url",
            "https://example.com/v1/messages"
        ]),
        |_| Some("placeholder".into())
    )
    .is_err());
    assert!(Config::resolve(args(&[]), |_| None).is_err());
}
