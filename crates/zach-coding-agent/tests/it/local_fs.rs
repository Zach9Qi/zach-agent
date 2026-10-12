//! 本地文件系统实现：基本读写、目录列举与遵守忽略规则的递归遍历

use std::io::ErrorKind;
use std::path::Path;
use tempfile::TempDir;
use zach_coding_agent::{FileSystem, LocalFileSystem};

/// 遍历结果中的相对路径（`/` 分隔、排序），目录带 `/` 后缀
async fn walked(fs: &LocalFileSystem, root: &Path) -> Vec<String> {
    let mut paths: Vec<String> = fs
        .walk(root)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| {
            let relative = entry.path.strip_prefix(root).unwrap();
            let relative = relative.to_string_lossy().replace('\\', "/");
            if entry.is_dir {
                format!("{relative}/")
            } else {
                relative
            }
        })
        .collect();
    paths.sort();
    paths
}

#[tokio::test]
async fn written_files_read_back_and_report_metadata() {
    let dir = TempDir::new().unwrap();
    let fs = LocalFileSystem;
    let nested = dir.path().join("a").join("b");
    fs.create_dir_all(&nested).await.unwrap();
    fs.create_dir_all(&nested).await.unwrap();
    let file = nested.join("f.txt");
    fs.write(&file, b"hello").await.unwrap();

    assert_eq!(fs.read(&file).await.unwrap(), b"hello");
    let info = fs.metadata(&file).await.unwrap();
    assert!(!info.is_dir);
    assert_eq!(info.len, 5);
    assert!(fs.metadata(&nested).await.unwrap().is_dir);
}

#[tokio::test]
async fn missing_paths_report_not_found() {
    let dir = TempDir::new().unwrap();
    let fs = LocalFileSystem;
    let missing = dir.path().join("missing");
    assert_eq!(
        fs.read(&missing).await.unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        fs.metadata(&missing).await.unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        fs.walk(&missing).await.unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

#[tokio::test]
async fn read_dir_lists_direct_children_including_hidden_ones() {
    let dir = TempDir::new().unwrap();
    let fs = LocalFileSystem;
    fs.create_dir_all(&dir.path().join("sub").join("deep"))
        .await
        .unwrap();
    fs.write(&dir.path().join(".env"), b"").await.unwrap();

    let mut entries: Vec<(String, bool)> = fs
        .read_dir(dir.path())
        .await
        .unwrap()
        .into_iter()
        .map(|entry| {
            let name = entry
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, entry.is_dir)
        })
        .collect();
    entries.sort();
    assert_eq!(entries, [(".env".into(), false), ("sub".into(), true)]);
}

/// 不在 git 仓库内也遵守 `.gitignore`，保留隐藏文件但跳过 `.git`
#[tokio::test]
async fn walk_honors_gitignore_and_skips_the_git_directory() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let fs = LocalFileSystem;
    for sub in ["src", "target", ".git", ".github"] {
        fs.create_dir_all(&root.join(sub)).await.unwrap();
    }
    fs.write(&root.join(".gitignore"), b"target/\n*.log\n")
        .await
        .unwrap();
    fs.write(&root.join("src").join("main.rs"), b"")
        .await
        .unwrap();
    fs.write(&root.join("debug.log"), b"").await.unwrap();
    fs.write(&root.join("target").join("out.bin"), b"")
        .await
        .unwrap();
    fs.write(&root.join(".git").join("HEAD"), b"")
        .await
        .unwrap();
    fs.write(&root.join(".github").join("ci.yml"), b"")
        .await
        .unwrap();

    assert_eq!(
        walked(&fs, root).await,
        [
            ".github/",
            ".github/ci.yml",
            ".gitignore",
            "src/",
            "src/main.rs"
        ]
    );
}
