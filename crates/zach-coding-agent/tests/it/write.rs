//! write 工具：新建与覆盖、自动创建父目录

use crate::support::{at, call, detail, text_of, tool, tools, MemoryFs};
use serde_json::json;

#[tokio::test]
async fn new_files_are_created_with_missing_parents() {
    let fs = MemoryFs::new();
    let write = tool(&tools(&fs).coding(), "write");
    let outcome = call(&write, json!({ "path": "a/b/new.txt", "content": "你好" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "已新建 a/b/new.txt（6 字节）");
    assert_eq!(detail(&outcome, "created"), true);
    assert_eq!(fs.text(&at("a/b/new.txt")).as_deref(), Some("你好"));
}

#[tokio::test]
async fn existing_files_are_overwritten() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "old");
    let write = tool(&tools(&fs).coding(), "write");
    let outcome = call(&write, json!({ "path": "a.txt", "content": "new" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "已覆盖 a.txt（3 字节）");
    assert_eq!(detail(&outcome, "created"), false);
    assert_eq!(fs.text(&at("a.txt")).as_deref(), Some("new"));
}

#[tokio::test]
async fn directories_cannot_be_overwritten() {
    let fs = MemoryFs::new();
    fs.put(&at("src/lib.rs"), "");
    let write = tool(&tools(&fs).coding(), "write");
    let error = call(&write, json!({ "path": "src", "content": "x" }))
        .await
        .unwrap_err();
    assert_eq!(error.message(), "src 是目录，无法写入");
}
