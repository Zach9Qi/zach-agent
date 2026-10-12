//! 文件存储：目录布局、末尾残行的忽略与修复、损坏与版本校验、ID 校验，以及换用新实例后的会话恢复

use super::{input, reply};
use crate::support::{reply_parts, ScriptedModel};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use zach_agent::Agent;
use zach_ai_core::{FileData, Message};
use zach_coding_agent::session::{
    BlobRef, JsonlStore, Session, SessionHeader, SessionId, SessionLog, SessionOptions,
    SessionStore, StoreError,
};

fn session_file(root: &Path, id: &str) -> PathBuf {
    root.join("sessions").join(format!("{id}.jsonl"))
}

/// 在会话文件末尾追加原始字节
fn append_raw(path: &Path, bytes: &str) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    file.write_all(bytes.as_bytes()).unwrap();
}

/// 创建会话并写入一问一答
async fn seeded(store: &JsonlStore, id: &str) -> SessionLog {
    let id = SessionId::new(id);
    store
        .create(&SessionHeader::new(id.clone(), None))
        .await
        .unwrap();
    let mut log = SessionLog::new();
    log.append(input("问"), 1).unwrap();
    log.append(reply("答"), 2).unwrap();
    store.append(&id, log.entries()).await.unwrap();
    log
}

#[tokio::test]
async fn files_follow_the_documented_layout() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlStore::new(dir.path());
    assert_eq!(store.root(), dir.path());
    seeded(&store, "s1").await;
    let blob = BlobRef::of(b"abc");
    store.put_blob(&blob, b"abc").await.unwrap();

    let text = std::fs::read_to_string(session_file(dir.path(), "s1")).unwrap();
    assert!(text.ends_with('\n'));
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["type"], "session");
    assert_eq!(lines[0]["id"], "s1");
    assert_eq!(lines[1]["id"], 1);
    assert_eq!(lines[2]["parent"], 1);

    let blobs: Vec<_> = std::fs::read_dir(dir.path().join("blobs"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(blobs, [blob.as_str()], "不应残留临时文件");
    assert_eq!(
        std::fs::read(dir.path().join("blobs").join(blob.as_str())).unwrap(),
        b"abc"
    );
}

/// 崩溃留下的残行：加载时忽略并警告；换一个实例追加前先截掉，新条目不会接在残行后面
#[tokio::test]
async fn a_partial_last_line_is_ignored_and_cut_before_the_next_append() {
    let dir = tempfile::tempdir().unwrap();
    let mut log = seeded(&JsonlStore::new(dir.path()), "s1").await;
    let path = session_file(dir.path(), "s1");
    append_raw(&path, r#"{"id":3,"interrupted_wri"#);

    let store = JsonlStore::new(dir.path());
    let id = SessionId::new("s1");
    let stored = store.load(&id).await.unwrap();
    assert_eq!(stored.entries, log.entries());
    assert_eq!(stored.warnings.len(), 1);
    assert!(
        stored.warnings[0].contains("第 4 行"),
        "{:?}",
        stored.warnings
    );

    let third = log.append(input("再问"), 3).unwrap().clone();
    store.append(&id, &[third]).await.unwrap();
    let stored = store.load(&id).await.unwrap();
    assert_eq!(stored.entries, log.entries());
    assert!(stored.warnings.is_empty());
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("interrupted_wri"));
}

/// 同一版本内出现未知的条目类型视为损坏，报告所在行号
#[tokio::test]
async fn corrupt_lines_report_their_line_number() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlStore::new(dir.path());
    seeded(&store, "s1").await;
    append_raw(
        &session_file(dir.path(), "s1"),
        "{\"id\":3,\"ts\":3,\"kind\":\"teleport\"}\n",
    );

    let error = store.load(&SessionId::new("s1")).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::Corrupt { line: 4, .. }),
        "{error}"
    );
}

#[tokio::test]
async fn files_without_a_complete_header_are_neither_loaded_nor_appended_to() {
    let dir = tempfile::tempdir().unwrap();
    let path = session_file(dir.path(), "s1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"type":"sess"#).unwrap();

    let store = JsonlStore::new(dir.path());
    let id = SessionId::new("s1");
    let mut log = SessionLog::new();
    let entry = log.append(input("问"), 1).unwrap().clone();
    assert!(matches!(
        store.load(&id).await,
        Err(StoreError::Corrupt { line: 1, .. })
    ));
    assert!(matches!(
        store.append(&id, &[entry]).await,
        Err(StoreError::Corrupt { line: 1, .. })
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"type":"sess"#);
}

#[tokio::test]
async fn headers_with_newer_versions_or_mismatched_ids_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    std::fs::write(
        session_file(dir.path(), "future"),
        "{\"type\":\"session\",\"format_version\":2,\"id\":\"future\",\"created_at\":1,\"unknown\":true}\n",
    )
    .unwrap();
    std::fs::write(
        session_file(dir.path(), "renamed"),
        "{\"type\":\"session\",\"format_version\":1,\"id\":\"original\",\"created_at\":1}\n",
    )
    .unwrap();

    let store = JsonlStore::new(dir.path());
    assert!(matches!(
        store.load(&SessionId::new("future")).await,
        Err(StoreError::UnsupportedVersion {
            version: 2,
            supported: 1,
            ..
        })
    ));
    assert!(matches!(
        store.load(&SessionId::new("renamed")).await,
        Err(StoreError::Corrupt { line: 1, .. })
    ));
}

#[tokio::test]
async fn ids_that_could_escape_the_root_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlStore::new(dir.path().join("root"));
    for raw in ["../escape", "a/b", "a\\b", "a.b", ""] {
        let id = SessionId::new(raw);
        let invalid =
            |result: Result<(), StoreError>| matches!(result, Err(StoreError::InvalidId(_)));
        assert!(
            invalid(store.create(&SessionHeader::new(id.clone(), None)).await),
            "{raw:?}"
        );
        assert!(invalid(store.append(&id, &[]).await), "{raw:?}");
        assert!(invalid(store.sync(&id).await), "{raw:?}");
        assert!(invalid(store.load(&id).await.map(drop)), "{raw:?}");
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn listing_skips_files_without_a_readable_header() {
    let dir = tempfile::tempdir().unwrap();
    let store = JsonlStore::new(dir.path());
    seeded(&store, "good").await;
    let sessions = dir.path().join("sessions");
    std::fs::write(sessions.join("bad.jsonl"), "不是 JSON\n").unwrap();
    std::fs::write(sessions.join("notes.txt"), "随手记录\n").unwrap();
    std::fs::write(
        sessions.join("moved.jsonl"),
        "{\"type\":\"session\",\"format_version\":1,\"id\":\"elsewhere\",\"created_at\":1}\n",
    )
    .unwrap();

    let ids: Vec<String> = store
        .list()
        .await
        .unwrap()
        .iter()
        .map(|header| header.id.to_string())
        .collect();
    assert_eq!(ids, ["good"]);
}

/// 换用新的存储实例（相当于进程重启）打开会话：上下文与大对象都从磁盘还原
#[tokio::test]
async fn a_session_resumes_from_disk_through_a_new_store() {
    let dir = tempfile::tempdir().unwrap();
    let model = ScriptedModel::new(vec![reply_parts("看到了"), reply_parts("还在")]);
    let agent = || Agent::builder(model.clone()).build();
    let options = SessionOptions::new().with_blob_threshold(16);
    let image = Message::user_with_file("看图", "image/png", FileData::from_bytes(vec![7u8; 64]));

    let session = Session::create(
        Arc::new(JsonlStore::new(dir.path())),
        agent(),
        options.clone(),
    );
    session
        .prompt(vec![image.clone()])
        .unwrap()
        .outcome()
        .await
        .unwrap();
    let id = session.id().clone();
    drop(session);
    assert_eq!(
        std::fs::read_dir(dir.path().join("blobs")).unwrap().count(),
        1
    );

    let store = Arc::new(JsonlStore::new(dir.path()));
    let reopened = Session::open(store, &id, agent(), options).await.unwrap();
    assert!(reopened.warnings().is_empty());
    assert_eq!(
        reopened.context().await.unwrap(),
        [image.clone(), Message::assistant("看到了")]
    );
    reopened
        .prompt_text("图还在吗")
        .unwrap()
        .outcome()
        .await
        .unwrap();
    let last = model.requests().pop().unwrap();
    assert_eq!(last[0], image);
    assert_eq!(last[2], Message::user("图还在吗"));
}

#[cfg(unix)]
#[tokio::test]
async fn files_and_directories_are_private_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let store = JsonlStore::new(dir.path());
    seeded(&store, "s1").await;
    let blob = BlobRef::of(b"abc");
    store.put_blob(&blob, b"abc").await.unwrap();

    let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.path().join("sessions")), 0o700);
    assert_eq!(mode(session_file(dir.path(), "s1")), 0o600);
    assert_eq!(mode(dir.path().join("blobs")), 0o700);
    assert_eq!(mode(dir.path().join("blobs").join(blob.as_str())), 0o600);
}
