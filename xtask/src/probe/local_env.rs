//! 从仓库根目录读取本地配置，迭代解析 dotenv 而不修改进程环境。

use super::ProbeResult;
use std::{collections::HashMap, io::ErrorKind, path::Path};

#[derive(Default)]
pub(super) struct LocalEnv {
    values: HashMap<String, String>,
}

impl LocalEnv {
    pub(super) fn load() -> ProbeResult<Self> {
        // 从子目录运行 cargo xtask 时仍使用同一份仓库配置，不搜索父级个人目录。
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.env.local");
        match std::fs::read_to_string(path) {
            Ok(source) => Self::parse(&source),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("读取仓库根目录 .env.local 失败: {}", error.kind())),
        }
    }

    pub(super) fn get(
        &self,
        name: &str,
        env: impl FnOnce(&str) -> Option<String>,
    ) -> Option<String> {
        env(name).or_else(|| self.values.get(name).cloned())
    }

    fn parse(source: &str) -> ProbeResult<Self> {
        // Windows 编辑器可能保存 UTF-8 BOM；迭代接口不会自动移除它。
        let source = source.trim_start_matches('\u{feff}');
        let values = dotenvy::from_read_iter(source.as_bytes())
            .collect::<Result<HashMap<_, _>, _>>()
            // dotenv 的原始错误包含整行输入，不能直接输出，以免泄漏密钥。
            .map_err(|_| ".env.local 格式无效，请检查 KEY=VALUE、引号及变量引用".to_owned())?;
        Ok(Self { values })
    }
}

#[cfg(test)]
mod tests {
    //! 本地 dotenv 解析、变量覆盖和解析错误脱敏。

    use super::*;
    use crate::probe::{args::Args, config::Config};

    fn args(values: &[&str]) -> Args {
        let mut values = values.to_vec();
        values.extend(["--scenario", "test.json"]);
        Args::parse(
            &values
                .iter()
                .map(|value| (*value).into())
                .collect::<Vec<_>>(),
        )
        .unwrap()
        .unwrap()
    }

    fn resolve(values: &[&str], env: impl Fn(&str) -> Option<String>) -> ProbeResult<Config> {
        Config::resolve(args(values), env).map(|mut configs| configs.remove(0))
    }

    #[test]
    fn dotenv_handles_bom_crlf_export_quotes_and_comments() {
        let source = "\u{feff}# 本地配置\r\n\
            export PROBE_MODEL=\"model with spaces\"\r\n\
            PROBE_API_KEY='secret$literal#=value'\r\n\
            PROBE_BASE_URL=https://example.com/v1 # 根地址\r\n";
        let local = LocalEnv::parse(source).unwrap();
        assert_eq!(
            local.get("PROBE_MODEL", |_| None).as_deref(),
            Some("model with spaces")
        );
        assert_eq!(
            local.get("PROBE_API_KEY", |_| None).as_deref(),
            Some("secret$literal#=value")
        );
        assert_eq!(
            local.get("PROBE_BASE_URL", |_| None).as_deref(),
            Some("https://example.com/v1")
        );
    }

    #[test]
    fn file_only_configuration_can_create_a_probe_config() {
        let local = LocalEnv::parse("PROBE_MODEL=local-model\nPROBE_API_KEY=local-key\n").unwrap();
        let config = resolve(&[], |name| local.get(name, |_| None)).unwrap();
        assert_eq!(config.model, "local-model");
        assert_eq!(config.api_key, "local-key");
        assert_eq!(config.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn command_line_and_existing_environment_override_the_same_file_variable() {
        let local = LocalEnv::parse("PROBE_MODEL=file-model\nPROBE_API_KEY=file-key\n").unwrap();
        let env = |name: &str| match name {
            "PROBE_MODEL" => Some("env-model".into()),
            "PROBE_API_KEY" => Some("env-key".into()),
            _ => None,
        };
        let config = resolve(&[], |name| local.get(name, env)).unwrap();
        assert_eq!(config.model, "env-model");
        assert_eq!(config.api_key, "env-key");
        let config = resolve(&["--model", "cli-model"], |name| local.get(name, env)).unwrap();
        assert_eq!(config.model, "cli-model");
        // 显式留空不能悄悄使用文件里的另一组凭据。
        let empty = resolve(&[], |name| local.get(name, |_| Some(String::new())));
        assert!(empty.is_err());
    }

    #[test]
    fn custom_key_variables_work_from_the_file_without_provider_fallbacks() {
        let local = LocalEnv::parse(
            "PROBE_MODEL=local-model\nOPENAI_API_KEY=unused-key\nTEAM_KEY=team-key\n",
        )
        .unwrap();
        assert!(resolve(&[], |name| local.get(name, |_| None)).is_err());
        let config = resolve(&["--api-key-env", "TEAM_KEY"], |name| {
            local.get(name, |_| None)
        })
        .unwrap();
        assert_eq!(config.model, "local-model");
        assert_eq!(config.api_key, "team-key");
    }

    #[test]
    fn last_duplicate_wins_and_empty_file_keeps_environment_configuration() {
        let local = LocalEnv::parse("PROBE_MODEL=first\nPROBE_MODEL=last\n").unwrap();
        assert_eq!(local.get("PROBE_MODEL", |_| None).as_deref(), Some("last"));
        let empty = LocalEnv::parse("").unwrap();
        assert_eq!(
            empty
                .get("PROBE_MODEL", |_| Some("env-model".into()))
                .as_deref(),
            Some("env-model")
        );
    }

    #[test]
    fn malformed_secret_lines_are_not_echoed_in_errors() {
        let error = LocalEnv::parse("PROBE_API_KEY=\"private-secret\n")
            .err()
            .unwrap();
        assert!(error.contains(".env.local"));
        assert!(!error.contains("private-secret"));
    }
}
