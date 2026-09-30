//! 内部辅助：ID 生成、可取消等待与 panic 信息提取

use std::any::Any;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

/// 生成进程内唯一的 ID（时间戳 + 自增序号）
pub(crate) fn new_id(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{nanos:x}{seq:04x}")
}

/// 等待 `future`，运行被中止时丢弃它并返回 `None`
pub(crate) async fn cancellable<F: Future>(
    cancel: &CancellationToken,
    future: F,
) -> Option<F::Output> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        output = future => Some(output),
    }
}

/// 从 `catch_unwind` 捕获的 panic 载荷中提取可读信息
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "未知 panic".to_string()
    }
}
