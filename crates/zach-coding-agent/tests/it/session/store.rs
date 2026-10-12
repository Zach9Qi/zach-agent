//! 会话存储契约：以进程内存储验证创建、追加、加载、列举与按内容寻址的大对象

use super::{input, reply};
use zach_coding_agent::session::{
    BlobRef, MemoryStore, SessionHeader, SessionId, SessionLog, SessionStore, StoreError,
};

fn header(id: &str, created_at: u64) -> SessionHeader {
    SessionHeader {
        created_at,
        ..SessionHeader::new(SessionId::new(id), Some("/repo".into()))
    }
}

#[tokio::test]
async fn appended_entries_load_back_in_order() {
    let store = MemoryStore::new();
    let header = header("s1", 1);
    store.create(&header).await.unwrap();

    let mut log = SessionLog::new();
    let first = log.append(input("问"), 1).unwrap().clone();
    let second = log.append(reply("答"), 2).unwrap().clone();
    store.append(&header.id, &[first]).await.unwrap();
    store.append(&header.id, &[second]).await.unwrap();
    store.sync(&header.id).await.unwrap();

    let stored = store.load(&header.id).await.unwrap();
    assert_eq!(stored.header, header);
    assert_eq!(stored.entries, log.entries());
    assert!(stored.warnings.is_empty());
}

#[tokio::test]
async fn duplicate_creation_and_unknown_sessions_are_errors() {
    let store = MemoryStore::new();
    let header = header("s1", 1);
    store.create(&header).await.unwrap();
    assert!(matches!(
        store.create(&header).await,
        Err(StoreError::AlreadyExists(id)) if id == header.id
    ));

    let unknown = SessionId::new("nope");
    assert!(matches!(
        store.load(&unknown).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.append(&unknown, &[]).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.sync(&unknown).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn sessions_are_listed_newest_first() {
    let store = MemoryStore::new();
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
    assert_eq!(ids, ["new", "mid", "old"]);
}

#[tokio::test]
async fn blobs_are_addressed_by_their_sha256() {
    let blob = BlobRef::of(b"abc");
    assert_eq!(
        blob.as_str(),
        "sha256-ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(BlobRef::parse(blob.as_str()), Some(blob.clone()));
    assert_eq!(BlobRef::parse("sha256-xyz"), None);
    assert_eq!(BlobRef::parse("md5-00"), None);

    let store = MemoryStore::new();
    store.put_blob(&blob, b"abc").await.unwrap();
    store.put_blob(&blob, b"abc").await.unwrap();
    assert_eq!(store.get_blob(&blob).await.unwrap(), b"abc");

    let missing = BlobRef::of(b"other");
    assert!(matches!(
        store.get_blob(&missing).await,
        Err(StoreError::BlobNotFound(blob)) if blob == missing
    ));
}
