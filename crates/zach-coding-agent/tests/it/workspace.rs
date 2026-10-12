//! 工作区的路径解析与范围判定

use std::path::{Path, PathBuf};
use zach_coding_agent::Workspace;

/// 当前平台下的绝对根目录
fn root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("C:\\ws")
    } else {
        PathBuf::from("/ws")
    }
}

#[test]
fn relative_paths_resolve_against_the_root() {
    let workspace = Workspace::new(root());
    assert_eq!(
        workspace.resolve("src/main.rs"),
        root().join("src").join("main.rs")
    );
    assert_eq!(workspace.resolve("./a/../b.txt"), root().join("b.txt"));
    assert_eq!(workspace.resolve(""), root());
}

#[test]
fn mention_prefix_and_surrounding_whitespace_are_ignored() {
    let workspace = Workspace::new(root());
    assert_eq!(
        workspace.resolve("  @src/lib.rs "),
        root().join("src").join("lib.rs")
    );
}

#[test]
fn absolute_paths_are_kept_and_normalized() {
    let workspace = Workspace::new(root());
    let outside = root().parent().unwrap().join("other").join("x.txt");
    let raw = outside.parent().unwrap().join(".").join("x.txt");
    assert_eq!(workspace.resolve(&raw.to_string_lossy()), outside);
}

#[test]
fn parent_segments_can_leave_the_workspace() {
    let workspace = Workspace::new(root());
    let escaped = workspace.resolve("../secret.txt");
    assert!(!workspace.contains(&escaped));
    assert!(workspace.contains(&workspace.resolve("a/../b")));
    assert!(workspace.contains(workspace.root()));
}

/// 防止仅凭字符串前缀判定：`/ws-other` 不属于 `/ws`
#[test]
fn sibling_directories_sharing_a_prefix_are_outside() {
    let workspace = Workspace::new(root());
    let sibling = PathBuf::from(format!("{}-other", root().display())).join("a");
    assert!(!workspace.contains(&sibling));
}

#[cfg(windows)]
#[test]
fn windows_paths_compare_case_insensitively_and_accept_msys_drives() {
    let workspace = Workspace::new("C:\\Work\\Repo");
    assert!(workspace.contains(Path::new("c:\\work\\repo\\src")));
    assert_eq!(
        workspace.resolve("/c/Work/Repo/src/a.rs"),
        PathBuf::from("C:\\Work\\Repo\\src\\a.rs")
    );
}

#[cfg(unix)]
#[test]
fn unix_paths_compare_case_sensitively() {
    let workspace = Workspace::new("/Work/Repo");
    assert!(!workspace.contains(Path::new("/work/repo/src")));
}
