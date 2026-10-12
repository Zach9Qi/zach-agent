//! 工作区越界策略：默认读放行、写需审批，以及拒绝时的错误结果

use crate::support::{call, root, tool, MemoryFs};
use serde_json::json;
use std::sync::Arc;
use zach_coding_agent::{CodingTools, OutsideAccess, Workspace};

/// 工作区之外的一个绝对路径
fn outside() -> String {
    root()
        .parent()
        .unwrap()
        .join("elsewhere.txt")
        .display()
        .to_string()
}

#[test]
fn defaults_allow_outside_reads_but_ask_before_outside_writes() {
    let fs = MemoryFs::new();
    let tools = CodingTools::new(Workspace::new(root()), fs).coding();
    let read = tool(&tools, "read");
    let write = tool(&tools, "write");

    assert!(!read.needs_approval(&json!({ "path": outside() })));
    assert!(write.needs_approval(&json!({ "path": outside(), "content": "" })));
    assert!(!write.needs_approval(&json!({ "path": "inside.txt", "content": "" })));
}

#[test]
fn ask_policy_applies_to_reads_when_configured() {
    let workspace = Workspace::new(root()).with_read_outside(OutsideAccess::Ask);
    let tools = CodingTools::new(workspace, MemoryFs::new()).coding();
    assert!(tool(&tools, "read").needs_approval(&json!({ "path": outside() })));
    assert!(!tool(&tools, "read").needs_approval(&json!({ "path": "a.txt" })));
}

#[tokio::test]
async fn denied_paths_fail_without_touching_the_file_system() {
    let fs = MemoryFs::new();
    let workspace = Workspace::new(root()).with_write_outside(OutsideAccess::Deny);
    let tools = CodingTools::new(workspace, fs.clone() as Arc<_>).coding();
    let write = tool(&tools, "write");

    assert!(!write.needs_approval(&json!({ "path": outside(), "content": "x" })));
    let error = call(&write, json!({ "path": outside(), "content": "x" }))
        .await
        .unwrap_err();
    assert_eq!(
        error.message(),
        format!("{} 位于工作区之外，不允许写入", outside())
    );
    assert_eq!(fs.text(std::path::Path::new(&outside())), None);
}
