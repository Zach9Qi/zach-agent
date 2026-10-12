//! read 工具：分段读取、截断提示、图片附件与各类错误

use crate::support::{at, call, detail, text_of, tool, tools, MemoryFs};
use serde_json::json;
use zach_agent::{SharedTool, ToolErrorKind};
use zach_ai_core::{ToolResultContentBlock, ToolResultOutput};
use zach_coding_agent::TruncationLimits;

fn read_tool(fs: &std::sync::Arc<MemoryFs>) -> SharedTool {
    tool(&tools(fs).read_only(), "read")
}

/// `n` 行文本，每行为 `line i`
fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

#[tokio::test]
async fn small_files_are_returned_whole() {
    let fs = MemoryFs::new();
    fs.put(&at("src/a.txt"), "one\ntwo\n");
    let outcome = call(&read_tool(&fs), json!({ "path": "src/a.txt" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "one\ntwo");
    assert_eq!(
        outcome.details,
        Some(json!({ "path": "src/a.txt", "total_lines": 2, "start_line": 1, "end_line": 2 }))
    );
}

/// 模型看到的文本统一为 LF、去掉 BOM，便于原样拷贝到 edit 的 old_text
#[tokio::test]
async fn crlf_and_bom_are_hidden_from_the_model() {
    let fs = MemoryFs::new();
    fs.put(&at("win.txt"), "\u{feff}a\r\nb\r\n");
    let outcome = call(&read_tool(&fs), json!({ "path": "win.txt" }))
        .await
        .unwrap();
    assert_eq!(text_of(&outcome), "a\nb");
}

#[tokio::test]
async fn offset_and_limit_select_a_window_with_a_continuation_hint() {
    let fs = MemoryFs::new();
    fs.put(&at("big.txt"), numbered(10));
    let input = json!({ "path": "big.txt", "offset": 3, "limit": 2 });
    let outcome = call(&read_tool(&fs), input).await.unwrap();
    assert_eq!(
        text_of(&outcome),
        "line 3\nline 4\n\n[还有 6 行未显示。使用 offset=5 继续读取]"
    );
    assert_eq!(detail(&outcome, "end_line"), 4);
}

#[tokio::test]
async fn output_beyond_the_limits_is_truncated_with_a_hint() {
    let fs = MemoryFs::new();
    fs.put(&at("big.txt"), numbered(10));
    let limits = TruncationLimits {
        max_lines: 4,
        max_bytes: 1024,
    };
    let read = tool(&tools(&fs).with_limits(limits).read_only(), "read");
    let outcome = call(&read, json!({ "path": "big.txt", "offset": 2 }))
        .await
        .unwrap();
    assert!(
        text_of(&outcome).ends_with("line 5\n\n[显示第 2-5 行，共 10 行。使用 offset=6 继续读取]")
    );
    assert_eq!(detail(&outcome, "truncation")["truncated_by"], "lines");
}

#[tokio::test]
async fn a_single_overlong_line_is_reported_instead_of_cut() {
    let fs = MemoryFs::new();
    fs.put(&at("min.js"), "x".repeat(100));
    let limits = TruncationLimits {
        max_lines: 10,
        max_bytes: 50,
    };
    let read = tool(&tools(&fs).with_limits(limits).read_only(), "read");
    let outcome = call(&read, json!({ "path": "min.js" })).await.unwrap();
    assert!(text_of(&outcome).starts_with("[第 1 行有 100 字节，超过 50 字节"));
}

#[tokio::test]
async fn empty_files_and_offsets_past_the_end_are_explained() {
    let fs = MemoryFs::new();
    fs.put(&at("empty.txt"), "");
    fs.put(&at("two.txt"), "a\nb");
    let read = read_tool(&fs);

    let empty = call(&read, json!({ "path": "empty.txt" })).await.unwrap();
    assert_eq!(text_of(&empty), "（empty.txt 是空文件）");

    let past = call(&read, json!({ "path": "two.txt", "offset": 5 }))
        .await
        .unwrap();
    assert!(past.is_error());
    assert_eq!(text_of(&past), "offset 5 超出文件末尾（two.txt 共 2 行）");
}

#[tokio::test]
async fn images_are_returned_as_attachments() {
    let fs = MemoryFs::new();
    let png = b"\x89PNG\r\n\x1a\n\x00\x00rest".to_vec();
    fs.put(&at("logo.png"), &png);
    let outcome = call(&read_tool(&fs), json!({ "path": "logo.png" }))
        .await
        .unwrap();
    let ToolResultOutput::Content { value } = &outcome.output else {
        panic!("应为复合结果");
    };
    assert!(matches!(
        &value[1],
        ToolResultContentBlock::File { media_type, data, filename, .. }
            if media_type == "image/png"
                && data.as_bytes() == Some(png.as_slice())
                && filename.as_deref() == Some("logo.png")
    ));
    assert_eq!(detail(&outcome, "bytes"), png.len());
}

#[tokio::test]
async fn binary_files_directories_and_missing_paths_fail() {
    let fs = MemoryFs::new();
    fs.put(&at("app.exe"), b"MZ\x00\x01");
    fs.put(&at("src/lib.rs"), "");
    let read = read_tool(&fs);

    let binary = call(&read, json!({ "path": "app.exe" })).await.unwrap_err();
    assert_eq!(binary.message(), "app.exe 是二进制文件，无法以文本读取");

    let dir = call(&read, json!({ "path": "src" })).await.unwrap_err();
    assert_eq!(dir.message(), "src 是目录，请用 ls 查看其内容");

    let missing = call(&read, json!({ "path": "nope.txt" }))
        .await
        .unwrap_err();
    assert_eq!(missing.message(), "路径不存在: nope.txt");

    let invalid = call(&read, json!({ "offset": 1 })).await.unwrap_err();
    assert_eq!(invalid.kind(), ToolErrorKind::InvalidInput);
}
