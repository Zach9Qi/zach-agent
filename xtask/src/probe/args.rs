//! 联调命令参数解析，不读取环境变量，也不创建网络客户端。

use super::ProbeResult;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Args {
    values: HashMap<String, String>,
}

impl Args {
    pub(super) fn parse(args: &[String]) -> ProbeResult<Option<Self>> {
        if args.iter().any(|arg| arg == "--help" || arg == "-h") {
            return Ok(None);
        }
        let mut parsed = Self::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let (name, inline) = arg
                .split_once('=')
                .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
            if !matches!(
                name,
                "--protocol" | "--mode" | "--scenario" | "--model" | "--base-url" | "--api-key-env"
            ) {
                return Err(
                    "存在未知参数，请运行 cargo xtask probe --help；密钥应通过环境变量提供".into(),
                );
            }
            let key = name.trim_start_matches("--");
            if parsed.values.contains_key(key) {
                return Err(format!("参数 {name} 重复指定"));
            }
            let value = inline
                .or_else(|| args.next().map(String::as_str))
                .filter(|v| !v.trim().is_empty() && !v.starts_with("--"))
                .ok_or_else(|| format!("{name} 缺少参数值"))?;
            parsed.values.insert(key.into(), value.into());
        }
        Ok(Some(parsed))
    }

    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}
