//! ls 与 find 工具：排序、目录标记、数量上限与 glob 匹配规则

use crate::support::{at, call, detail, text_of, tool, tools, MemoryFs};
use serde_json::json;
use std::sync::Arc;
use zach_agent::{SharedTool, ToolErrorKind};
use zach_coding_agent::FileSystem;

/// 一个小型项目：src 下两层源码、根目录若干文件
fn project() -> Arc<MemoryFs> {
    let fs = MemoryFs::new();
    for path in [
        "README.md",
        ".env",
        "Cargo.toml",
        "src/lib.rs",
        "src/main.rs",
        "src/tools/read.rs",
        "crates/a/src/lib.rs",
    ] {
        fs.put(&at(path), "");
    }
    fs
}

fn named(fs: &Arc<MemoryFs>, name: &str) -> SharedTool {
    tool(&tools(fs).read_only(), name)
}

#[tokio::test]
async fn ls_sorts_entries_case_insensitively_and_marks_directories() {
    let fs = project();
    let outcome = call(&named(&fs, "ls"), json!({})).await.unwrap();
    assert_eq!(
        text_of(&outcome),
        ".env\nCargo.toml\ncrates/\nREADME.md\nsrc/"
    );
    assert_eq!(detail(&outcome, "entries"), 5);

    let outcome = call(&named(&fs, "ls"), json!({ "path": "src" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "lib.rs\nmain.rs\ntools/");
}

#[tokio::test]
async fn ls_limit_reports_how_many_entries_were_hidden() {
    let fs = project();
    let outcome = call(&named(&fs, "ls"), json!({ "limit": 2 }))
        .await
        .unwrap();
    assert_eq!(
        text_of(&outcome),
        ".env\nCargo.toml\n\n[共 5 项，只显示前 2 项。可用 limit 调整，或用 find 按模式查找]"
    );
}

#[tokio::test]
async fn ls_explains_empty_and_missing_directories() {
    let fs = MemoryFs::new();
    fs.create_dir_all(&at("empty")).await.unwrap();
    let ls = named(&fs, "ls");

    let outcome = call(&ls, json!({ "path": "empty" })).await.unwrap();
    assert_eq!(text_of(&outcome), "（empty 是空目录）");
    let missing = call(&ls, json!({ "path": "nothing" })).await.unwrap_err();
    assert_eq!(missing.message(), "路径不存在: nothing");
}

#[tokio::test]
async fn find_matches_file_names_at_any_depth() {
    let fs = project();
    let outcome = call(&named(&fs, "find"), json!({ "pattern": "lib.rs" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "crates/a/src/lib.rs\nsrc/lib.rs");
    assert_eq!(detail(&outcome, "matches"), 2);
}

#[tokio::test]
async fn find_path_patterns_match_at_any_depth_unless_anchored() {
    let fs = project();
    let find = named(&fs, "find");

    let nested = call(&find, json!({ "pattern": "src/*.rs" })).await.unwrap();
    assert_eq!(
        text_of(&nested),
        "crates/a/src/lib.rs\nsrc/lib.rs\nsrc/main.rs"
    );

    let anchored = call(&find, json!({ "pattern": "/src/*.rs" }))
        .await
        .unwrap();
    assert_eq!(text_of(&anchored), "src/lib.rs\nsrc/main.rs");

    let scoped = call(&find, json!({ "pattern": "*.rs", "path": "src/tools" }))
        .await
        .unwrap();
    assert_eq!(text_of(&scoped), "src/tools/read.rs");

    let dirs = call(&find, json!({ "pattern": "src" })).await.unwrap();
    assert_eq!(text_of(&dirs), "crates/a/src/\nsrc/");
}

#[tokio::test]
async fn find_reports_no_matches_limits_and_invalid_patterns() {
    let fs = project();
    let find = named(&fs, "find");

    let none = call(&find, json!({ "pattern": "*.py" })).await.unwrap();
    assert_eq!(text_of(&none), "在 . 下没有匹配 *.py 的路径");

    let limited = call(&find, json!({ "pattern": "*.rs", "limit": 1 }))
        .await
        .unwrap();
    assert_eq!(
        text_of(&limited),
        "crates/a/src/lib.rs\n\n[共 4 个结果，只显示前 1 个。可缩小搜索范围或用 limit 调整]"
    );

    let invalid = call(&find, json!({ "pattern": "src/[a" }))
        .await
        .unwrap_err();
    assert_eq!(invalid.kind(), ToolErrorKind::InvalidInput);
}
