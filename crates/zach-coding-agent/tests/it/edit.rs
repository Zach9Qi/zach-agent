//! edit 工具：多处替换、换行符与 BOM 往返、差异详情、入参兼容与失败提示

use crate::support::{at, call, detail, text_of, tool, tools, MemoryFs};
use serde_json::{json, Value};
use std::sync::Arc;
use zach_agent::{SharedTool, ToolErrorKind};

fn edit_tool(fs: &Arc<MemoryFs>) -> SharedTool {
    tool(&tools(fs).coding(), "edit")
}

fn edits(pairs: &[(&str, &str)]) -> Value {
    pairs
        .iter()
        .map(|(old, new)| json!({ "old_text": old, "new_text": new }))
        .collect()
}

#[tokio::test]
async fn multiple_edits_apply_against_the_original_file() {
    let fs = MemoryFs::new();
    fs.put(&at("src/lib.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n");
    let input = json!({
        "path": "src/lib.rs",
        "edits": edits(&[("fn c() {}", "fn c2() {}"), ("fn a() {}", "fn a2() {}")]),
    });
    let outcome = call(&edit_tool(&fs), input).await.unwrap();
    assert_eq!(text_of(&outcome), "已在 src/lib.rs 中替换 2 处");
    assert_eq!(
        fs.text(&at("src/lib.rs")).as_deref(),
        Some("fn a2() {}\nfn b() {}\nfn c2() {}\n")
    );
}

#[tokio::test]
async fn details_carry_a_unified_diff_and_the_first_changed_line() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "1\n2\n3\n4\n");
    let input = json!({ "path": "a.txt", "edits": edits(&[("3", "three")]) });
    let outcome = call(&edit_tool(&fs), input).await.unwrap();
    assert_eq!(detail(&outcome, "first_changed_line"), 3);
    assert_eq!(detail(&outcome, "replacements"), 1);
    assert_eq!(
        detail(&outcome, "diff"),
        "--- a.txt\n+++ a.txt\n@@ -1,4 +1,4 @@\n 1\n 2\n-3\n+three\n 4\n"
    );
}

/// 模型给的 old_text 是 LF，文件是 CRLF + BOM：照样匹配，写回保持原格式
#[tokio::test]
async fn crlf_and_bom_survive_the_round_trip() {
    let fs = MemoryFs::new();
    fs.put(&at("win.txt"), "\u{feff}one\r\ntwo\r\nthree\r\n");
    let input = json!({ "path": "win.txt", "edits": edits(&[("one\ntwo", "uno\ndos")]) });
    call(&edit_tool(&fs), input).await.unwrap();
    assert_eq!(
        fs.text(&at("win.txt")).as_deref(),
        Some("\u{feff}uno\r\ndos\r\nthree\r\n")
    );
}

#[tokio::test]
async fn fuzzy_matches_are_applied_and_reported() {
    let fs = MemoryFs::new();
    fs.put(&at("q.py"), "print(“hi”)  \nend\n");
    let input = json!({ "path": "q.py", "edits": edits(&[("print(\"hi\")", "print(\"bye\")")]) });
    let outcome = call(&edit_tool(&fs), input).await.unwrap();
    assert!(text_of(&outcome).contains("其中 1 处为宽松匹配"));
    assert_eq!(detail(&outcome, "fuzzy_matches"), 1);
    assert_eq!(
        fs.text(&at("q.py")).as_deref(),
        Some("print(\"bye\")  \nend\n")
    );
}

/// 兼容单个对象与被编码成字符串的数组
#[tokio::test]
async fn malformed_but_recoverable_edit_shapes_are_accepted() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "x y");
    let edit = edit_tool(&fs);

    let single = json!({ "path": "a.txt", "edits": { "old_text": "x", "new_text": "1" } });
    call(&edit, single).await.unwrap();
    let encoded = edits(&[("y", "2")]).to_string();
    call(&edit, json!({ "path": "a.txt", "edits": encoded }))
        .await
        .unwrap();
    assert_eq!(fs.text(&at("a.txt")).as_deref(), Some("1 2"));

    let garbage = call(&edit, json!({ "path": "a.txt", "edits": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(garbage.kind(), ToolErrorKind::InvalidInput);
    let empty = call(&edit, json!({ "path": "a.txt", "edits": [] }))
        .await
        .unwrap_err();
    assert_eq!(empty.message(), "edits 不能为空");
}

/// 任何一处失败都不写回，错误信息指出是哪一项
#[tokio::test]
async fn failures_leave_the_file_untouched() {
    let fs = MemoryFs::new();
    fs.put(&at("a.txt"), "dup dup once");
    let edit = edit_tool(&fs);

    let input = json!({ "path": "a.txt", "edits": edits(&[("once", "1"), ("dup", "2")]) });
    let error = call(&edit, input).await.unwrap_err();
    assert_eq!(
        error.message(),
        "a.txt: edits[1].old_text 在文件中出现了 2 次，必须唯一。请包含更多上下文"
    );
    assert_eq!(fs.text(&at("a.txt")).as_deref(), Some("dup dup once"));

    let input = json!({ "path": "a.txt", "edits": edits(&[("missing", "1")]) });
    let error = call(&edit, input).await.unwrap_err();
    assert!(error
        .message()
        .starts_with("a.txt: 找不到 edits[0].old_text"));
}

#[tokio::test]
async fn binary_invalid_utf8_and_missing_files_are_rejected() {
    let fs = MemoryFs::new();
    fs.put(&at("bin"), b"a\x00b");
    fs.put(&at("latin1.txt"), b"caf\xe9");
    let edit = edit_tool(&fs);
    let input = |path: &str| json!({ "path": path, "edits": edits(&[("a", "b")]) });

    let binary = call(&edit, input("bin")).await.unwrap_err();
    assert_eq!(binary.message(), "bin 是二进制文件，无法编辑");
    let latin1 = call(&edit, input("latin1.txt")).await.unwrap_err();
    assert_eq!(
        latin1.message(),
        "latin1.txt 不是有效的 UTF-8 文本，无法编辑"
    );
    let missing = call(&edit, input("nope.txt")).await.unwrap_err();
    assert_eq!(missing.message(), "路径不存在: nope.txt");
}

#[test]
fn schema_declares_edits_as_an_array_of_pairs() {
    let fs = MemoryFs::new();
    let edit = edit_tool(&fs);
    let schema = &edit.definition().input_schema;
    let edits = &schema["properties"]["edits"];
    assert_eq!(edits["type"], "array");
    assert_eq!(edits["items"]["required"], json!(["old_text", "new_text"]));
}
