//! 构建脚本：扫描 `data/models/*.json`，生成内置模型档案的嵌入清单
//!
//! 数据文件由 `cargo xtask sync-models` 维护，这里只负责"目录里有什么就嵌入什么"，
//! 避免在源码中再手工维护一份厂商列表。

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let data_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("data/models");
    // 目录下任何文件增删改都触发重新生成
    println!("cargo:rerun-if-changed={}", data_dir.display());

    let mut files: Vec<PathBuf> = fs::read_dir(&data_dir)
        .unwrap_or_else(|err| panic!("读取 {} 失败: {err}", data_dir.display()))
        .map(|entry| entry.expect("读取目录项失败").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();

    // 生成形如 `&[("anthropic", include_str!("<绝对路径>")), ...]` 的表达式
    let mut code = String::from("&[\n");
    for path in &files {
        let provider = path.file_stem().unwrap().to_string_lossy();
        let path = path.to_string_lossy();
        code.push_str(&format!("    ({provider:?}, include_str!({path:?})),\n"));
    }
    code.push_str("]\n");

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin_sources.rs");
    fs::write(&out, code).unwrap_or_else(|err| panic!("写入 {} 失败: {err}", out.display()));
}
