//! 命令行解析、配置优先级和凭据来源隔离。

use super::super::{
    args::Args,
    config::{Config, Mode, Protocol},
};
use super::support::{args, config};

#[test]
fn help_and_invalid_protocol_do_not_read_credentials() {
    assert!(Args::parse(&["--help".into()]).unwrap().is_none());
    let result = Config::resolve(args(&["--protocol", "openai-chat"]), |_| {
        panic!("未实现的协议不应读取凭据")
    });
    assert!(matches!(result, Err(error) if error.contains("尚未实现")));
}

#[test]
fn command_line_overrides_generic_environment_and_generic_overrides_provider_environment() {
    let env = |key: &str| match key {
        "PROBE_MODEL" => Some("generic-model".into()),
        "OPENAI_MODEL" => Some("provider-model".into()),
        "PROBE_BASE_URL" => Some("https://generic.example/v1/".into()),
        "OPENAI_BASE_URL" => Some("https://provider.example/v1".into()),
        "PROBE_API_KEY" => Some("generic-key".into()),
        "OPENAI_API_KEY" => Some("provider-key".into()),
        _ => None,
    };
    let selected = Config::resolve(args(&[]), env).unwrap();
    assert_eq!(selected.protocol, Protocol::OpenAiResponses);
    assert_eq!(selected.mode, Mode::Stream);
    assert_eq!(selected.model, "generic-model");
    assert_eq!(selected.base_url, "https://generic.example/v1");
    assert_eq!(selected.api_key, "generic-key");
    let selected = Config::resolve(
        args(&[
            "--model=cli-model",
            "--base-url",
            "https://cli.example/v2",
            "--mode=generate",
        ]),
        env,
    )
    .unwrap();
    assert_eq!(selected.model, "cli-model");
    assert_eq!(selected.base_url, "https://cli.example/v2");
    assert_eq!(selected.mode, Mode::Generate);
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
fn responses_accepts_existing_openai_environment_variables() {
    let config = Config::resolve(args(&[]), |key| match key {
        "OPENAI_MODEL" => Some("provider-model".into()),
        "OPENAI_API_KEY" => Some("provider-key".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(config.model, "provider-model");
    assert_eq!(config.api_key, "provider-key");
    assert_eq!(config.base_url, "https://api.openai.com/v1");
}

#[test]
fn invalid_flags_ranges_and_sensitive_urls_fail_before_running() {
    for values in [
        vec!["--raw=true"],
        vec!["--model"],
        vec!["--model", "--raw"],
        vec!["--mode", "stream", "--mode", "generate"],
        vec!["--api-key=secret"],
    ] {
        assert!(Args::parse(&values.iter().map(|s| (*s).into()).collect::<Vec<_>>()).is_err());
    }
    for values in [
        vec!["--timeout-secs", "0"],
        vec!["--timeout-secs", "3601"],
        vec!["--mode", "agent", "--max-steps", "1"],
        vec!["--mode", "invalid"],
        vec!["--max-steps", "2"],
        vec!["--max-output-tokens", "4294967296"],
        vec!["--base-url", "https://user:secret@example.com/v1"],
        vec!["--base-url", "https://example.com/v1?key=secret"],
        vec!["--base-url", "https://example.com/v1/responses"],
    ] {
        assert!(Config::resolve(args(&values), |_| Some("placeholder".into())).is_err());
    }
    assert!(Config::resolve(args(&[]), |_| None).is_err());
}

#[test]
fn prompt_raw_output_and_token_limit_reach_call_options() {
    let config = config(&["--prompt", "你好", "--raw", "--max-output-tokens", "2048"]);
    assert_eq!(config.prompt, "你好");
    assert!(config.options.include_raw_chunks);
    assert_eq!(config.options.max_output_tokens, Some(2048));
}
