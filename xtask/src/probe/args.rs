//! 联调命令参数解析，不读取环境变量，也不创建网络客户端。

use super::ProbeResult;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct Args {
    pub(super) values: HashMap<String, String>,
    pub(super) raw: bool,
}

impl Args {
    pub(super) fn parse(args: &[String]) -> ProbeResult<Option<Self>> {
        if args.iter().any(|arg| arg == "--help" || arg == "-h") {
            return Ok(None);
        }
        let mut parsed = Self::default();
        let mut seen = HashSet::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
            if !matches!(
                name,
                "--protocol"
                    | "--mode"
                    | "--model"
                    | "--base-url"
                    | "--api-key-env"
                    | "--prompt"
                    | "--timeout-secs"
                    | "--max-steps"
                    | "--max-output-tokens"
                    | "--raw"
            ) {
                return Err(
                    "存在未知参数，请运行 cargo xtask probe --help；密钥应通过环境变量提供".into(),
                );
            }
            if !seen.insert(name.to_owned()) {
                return Err(format!("参数 {name} 重复指定"));
            }
            if name == "--raw" {
                if inline.is_some() {
                    return Err("--raw 不接受参数值".into());
                }
                parsed.raw = true;
                continue;
            }
            let value = inline
                .or_else(|| args.next().map(String::as_str))
                .filter(|v| !v.trim().is_empty() && !v.starts_with("--"))
                .ok_or_else(|| format!("{name} 缺少参数值"))?;
            parsed
                .values
                .insert(name.trim_start_matches("--").into(), value.into());
        }
        Ok(Some(parsed))
    }

    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}
