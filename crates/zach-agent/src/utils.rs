//! 内部辅助：ID 生成、用量累加、可取消等待与 panic 信息提取

use std::any::Any;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;
use zach_ai_core::Usage;

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

/// 把 `usage` 累加进 `total`；厂商原始数据不参与累加
pub(crate) fn add_usage(total: &mut Usage, usage: &Usage) {
    add(&mut total.input_tokens.total, usage.input_tokens.total);
    add(
        &mut total.input_tokens.no_cache,
        usage.input_tokens.no_cache,
    );
    add(
        &mut total.input_tokens.cache_read,
        usage.input_tokens.cache_read,
    );
    add(
        &mut total.input_tokens.cache_write,
        usage.input_tokens.cache_write,
    );
    add(&mut total.output_tokens.total, usage.output_tokens.total);
    add(&mut total.output_tokens.text, usage.output_tokens.text);
    add(
        &mut total.output_tokens.reasoning,
        usage.output_tokens.reasoning,
    );
}

fn add(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0) + value);
    }
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
