//! 会话存储契约：进程内存储与文件存储都须满足的创建、追加、加载、列举与按内容寻址的大对象

use super::{input, reply};
use tempfile::TempDir;
use zach_coding_agent::session::{
    BlobRef, JsonlStore, MemoryStore, SessionHeader, SessionId, SessionLog, SessionStore,
    StoreError,
};

/// 被测的存储实现；文件存储的临时目录随之保存，用例结束时才删除
struct Subject {
    name: &'static str,
    store: Box<dyn SessionStore>,
    _dir: Option<TempDir>,
}

fn subjects() -> Vec<Subject> {
    let dir = tempfile::tempdir().unwrap();
    vec![
        Subject {
            name: "memory",
            store: Box::new(MemoryStore::new()),
            _dir: None,
        },
        Subject {
            name: "jsonl",
            store: Box::new(JsonlStore::new(dir.path())),
            _dir: Some(dir),
        },
    ]
}

fn header(id: &str, created_at: u64) -> SessionHeader {
    SessionHeader {
        created_at,
        ..SessionHeader::new(SessionId::new(id), Some("/repo".into()))
    }
}

#[tokio::test]
async fn appended_entries_load_back_in_order() {
    for subject in subjects() {
        let (name, store) = (subject.name, subject.store.as_ref());
        let header = header("s1", 1);
        store.create(&header).await.unwrap();

        let mut log = SessionLog::new();
        let first = log.append(input("问"), 1).unwrap().clone();
        let second = log.append(reply("答"), 2).unwrap().clone();
        store.append(&header.id, &[first]).await.unwrap();
        store.append(&header.id, &[]).await.unwrap();
        store.append(&header.id, &[second]).await.unwrap();
        store.sync(&header.id).await.unwrap();

        let stored = store.load(&header.id).await.unwrap();
        assert_eq!(stored.header, header, "{name}");
        assert_eq!(stored.entries, log.entries(), "{name}");
        assert!(stored.warnings.is_empty(), "{name}");
    }
}

#[tokio::test]
async fn duplicate_creation_and_unknown_sessions_are_errors() {
    for subject in subjects() {
        let (name, store) = (subject.name, subject.store.as_ref());
        let header = header("s1", 1);
        store.create(&header).await.unwrap();
        assert!(
            matches!(
                store.create(&header).await,
                Err(StoreError::AlreadyExists(id)) if id == header.id
            ),
            "{name}"
        );

        let unknown = SessionId::new("nope");
        assert!(
            matches!(store.load(&unknown).await, Err(StoreError::NotFound(_))),
            "{name}"
        );
        assert!(
            matches!(
                store.append(&unknown, &[]).await,
                Err(StoreError::NotFound(_))
            ),
            "{name}"
        );
        assert!(
            matches!(store.sync(&unknown).await, Err(StoreError::NotFound(_))),
            "{name}"
        );
    }
}

#[tokio::test]
async fn sessions_are_listed_newest_first() {
    for subject in subjects() {
        let (name, store) = (subject.name, subject.store.as_ref());
        assert!(store.list().await.unwrap().is_empty(), "{name}");
        for (id, created_at) in [("old", 1), ("new", 3), ("mid", 2)] {
            store.create(&header(id, created_at)).await.unwrap();
        }
        let ids: Vec<String> = store
            .list()
            .await
            .unwrap()
            .iter()
            .map(|header| header.id.to_string())
            .collect();
        assert_eq!(ids, ["new", "mid", "old"], "{name}");
    }
}

#[test]
fn blob_references_are_the_sha256_of_the_content() {
    let blob = BlobRef::of(b"abc");
    assert_eq!(
        blob.as_str(),
        "sha256-ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(BlobRef::parse(blob.as_str()), Some(blob.clone()));
    assert_eq!(BlobRef::parse("sha256-xyz"), None);
    assert_eq!(BlobRef::parse("md5-00"), None);
}

#[tokio::test]
async fn blobs_are_stored_once_and_read_back() {
    for subject in subjects() {
        let (name, store) = (subject.name, subject.store.as_ref());
        let blob = BlobRef::of(b"abc");
        store.put_blob(&blob, b"abc").await.unwrap();
        store.put_blob(&blob, b"abc").await.unwrap();
        assert_eq!(store.get_blob(&blob).await.unwrap(), b"abc", "{name}");

        let missing = BlobRef::of(b"other");
        assert!(
            matches!(
                store.get_blob(&missing).await,
                Err(StoreError::BlobNotFound(blob)) if blob == missing
            ),
            "{name}"
        );
    }
}
