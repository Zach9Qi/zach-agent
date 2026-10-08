//! 从 Rust 结果类型生成可复用的联调输出 Schema，整个过程不访问网络。

mod types;

#[cfg(test)]
mod tests;

use serde_json::Value;
use std::{error::Error, path::Path};

pub(crate) fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    match args {
        [] => {}
        [flag] if flag == "--help" || flag == "-h" => {
            println!("用法: cargo xtask probe-schemas\n从 xtask/src/probe_schemas/types.rs 生成 scenarios/probe/schemas/*.json；无需密钥或网络。");
            return Ok(());
        }
        _ => return Err("未知参数，请运行 cargo xtask probe-schemas --help".into()),
    }
    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios/probe/schemas");
    std::fs::create_dir_all(&output)?;
    for (name, schema) in schemas() {
        let text = serde_json::to_string_pretty(&schema)? + "\n";
        let path = output.join(name);
        std::fs::write(&path, text)?;
        println!("已生成 {}", path.display());
    }
    Ok(())
}

/// 类型到文件名的唯一登记处；输出只描述结构，不包含场景的预期答案。
fn schemas() -> [(&'static str, Value); 1] {
    [(
        "attachment-summary.json",
        schemars::schema_for!(types::AttachmentSummary).to_value(),
    )]
}
