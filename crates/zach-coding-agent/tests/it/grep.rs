//! grep 工具：正则与字面量、文件过滤、上下文、匹配上限与跳过二进制文件

use crate::support::{at, call, detail, text_of, tool, tools, MemoryFs};
use serde_json::json;
use std::sync::Arc;
use zach_agent::{SharedTool, ToolErrorKind};

/// 两个源码文件、一份文档和一个二进制文件
fn project() -> Arc<MemoryFs> {
    let fs = MemoryFs::new();
    fs.put(
        &at("src/lib.rs"),
        "pub fn alpha() {}\n// TODO: beta\nfn gamma() {}\n",
    );
    fs.put(&at("src/main.rs"), "fn main() {\n    alpha();\n}\n");
    fs.put(
        &at("README.md"),
        "Alpha docs\na.b literal\naxb regex only\n",
    );
    fs.put(&at("logo.bin"), b"alpha\x00\x01");
    fs
}

fn grep(fs: &Arc<MemoryFs>) -> SharedTool {
    tool(&tools(fs).read_only(), "grep")
}

#[tokio::test]
async fn regex_matches_are_listed_by_path_and_line() {
    let fs = project();
    let outcome = call(&grep(&fs), json!({ "pattern": r"alpha\(" }))
        .await
        .unwrap();
    assert_eq!(
        text_of(&outcome),
        "src/lib.rs:1: pub fn alpha() {}\nsrc/main.rs:2:     alpha();"
    );
    assert_eq!(detail(&outcome, "matches"), 2);
    assert_eq!(detail(&outcome, "files"), 2);
}

/// 二进制文件即使包含匹配文本也不出现在结果中
#[tokio::test]
async fn case_folding_includes_more_files_but_skips_binaries() {
    let fs = project();
    let outcome = call(
        &grep(&fs),
        json!({ "pattern": "alpha", "ignore_case": true }),
    )
    .await
    .unwrap();
    assert_eq!(
        text_of(&outcome),
        "README.md:1: Alpha docs\nsrc/lib.rs:1: pub fn alpha() {}\nsrc/main.rs:2:     alpha();"
    );
}

#[tokio::test]
async fn literal_mode_escapes_regex_syntax() {
    let fs = project();
    let regex = call(&grep(&fs), json!({ "pattern": "a.b" })).await.unwrap();
    assert_eq!(detail(&regex, "matches"), 2);

    let literal = call(&grep(&fs), json!({ "pattern": "a.b", "literal": true }))
        .await
        .unwrap();
    assert_eq!(text_of(&literal), "README.md:2: a.b literal");
}

#[tokio::test]
async fn glob_and_path_narrow_the_searched_files() {
    let fs = project();
    let globbed = call(&grep(&fs), json!({ "pattern": "fn", "glob": "main.rs" }))
        .await
        .unwrap();
    assert_eq!(text_of(&globbed), "src/main.rs:1: fn main() {");

    let single = call(
        &grep(&fs),
        json!({ "pattern": "TODO", "path": "src/lib.rs" }),
    )
    .await
    .unwrap();
    assert_eq!(text_of(&single), "src/lib.rs:2: // TODO: beta");
}

#[tokio::test]
async fn context_lines_surround_matches_with_separators_between_files() {
    let fs = project();
    let input = json!({ "pattern": "alpha", "glob": "*.rs", "context": 1 });
    let outcome = call(&grep(&fs), input).await.unwrap();
    assert_eq!(
        text_of(&outcome),
        "src/lib.rs:1: pub fn alpha() {}\nsrc/lib.rs-2- // TODO: beta\n--\n\
         src/main.rs-1- fn main() {\nsrc/main.rs:2:     alpha();\nsrc/main.rs-3- }"
    );
}

#[tokio::test]
async fn match_limit_stops_the_search_with_a_hint() {
    let fs = project();
    let outcome = call(&grep(&fs), json!({ "pattern": "fn", "limit": 2 }))
        .await
        .unwrap();
    assert_eq!(
        text_of(&outcome),
        "src/lib.rs:1: pub fn alpha() {}\nsrc/lib.rs:3: fn gamma() {}\n\n\
         [已达到 2 条匹配的上限，可用 limit 调整或细化模式]"
    );
    assert_eq!(detail(&outcome, "limit_reached"), true);
}

#[tokio::test]
async fn no_matches_and_invalid_patterns_are_reported() {
    let fs = project();
    let none = call(&grep(&fs), json!({ "pattern": "zeta" }))
        .await
        .unwrap();
    assert_eq!(text_of(&none), "在 . 中没有找到匹配 zeta 的内容");

    let regex = call(&grep(&fs), json!({ "pattern": "(" }))
        .await
        .unwrap_err();
    assert_eq!(regex.kind(), ToolErrorKind::InvalidInput);
    let glob = call(&grep(&fs), json!({ "pattern": "x", "glob": "[" }))
        .await
        .unwrap_err();
    assert_eq!(glob.kind(), ToolErrorKind::InvalidInput);
}
