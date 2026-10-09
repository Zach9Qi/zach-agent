//! 联调命令参数解析：位置参数 `<协议> [模式]` 与同名 flag 等价，不读取环境变量。

use super::ProbeResult;
use std::collections::HashMap;

/// 位置参数按顺序对应的键；与 `--protocol` / `--mode` 共用同一存储，重复指定即报错。
const POSITIONAL: [&str; 2] = ["protocol", "mode"];

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
        let mut positional = POSITIONAL.iter();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if !arg.starts_with("--") {
                let key = positional
                    .next()
                    .ok_or("位置参数最多两个：<协议> [模式]，其余请使用 --选项")?;
                parsed.insert(key, arg)?;
                continue;
            }
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
            let value = inline
                .or_else(|| args.next().map(String::as_str))
                .filter(|v| !v.trim().is_empty() && !v.starts_with("--"))
                .ok_or_else(|| format!("{name} 缺少参数值"))?;
            parsed.insert(key, value)?;
        }
        Ok(Some(parsed))
    }

    fn insert(&mut self, key: &str, value: &str) -> ProbeResult<()> {
        if value.trim().is_empty() {
            return Err(format!("参数 {key} 不能为空"));
        }
        if self.values.insert(key.into(), value.into()).is_some() {
            return Err(format!(
                "参数 {key} 重复指定（位置参数与 --{key} 只能二选一）"
            ));
        }
        Ok(())
    }

    pub(super) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}
