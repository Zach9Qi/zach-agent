//! 落盘格式：第一行文件头，之后每行一个条目，每行都以换行符结尾
//!
//! 没有换行符结尾的末尾残行视为写入时进程中断留下的，加载时忽略并给出警告。

use crate::session::entry::{Entry, SessionHeader, SessionId, FORMAT_VERSION};
use crate::session::store::{StoreError, StoredSession};
use serde_json::Value;
use std::io;

/// 文件头编码为一行
pub(super) fn encode_header(header: &SessionHeader) -> io::Result<Vec<u8>> {
    let mut line = serde_json::to_vec(header)?;
    line.push(b'\n');
    Ok(line)
}

/// 一批条目编码为连续的行
pub(super) fn encode_entries(entries: &[Entry]) -> io::Result<Vec<u8>> {
    let mut lines = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut lines, entry)?;
        lines.push(b'\n');
    }
    Ok(lines)
}

/// 到最后一个换行符（含）为止的长度，即完整行的部分
pub(super) fn complete_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1)
}

/// 解析会话文件的全部内容
pub(super) fn parse(id: &SessionId, bytes: &[u8]) -> Result<StoredSession, StoreError> {
    let complete = complete_len(bytes);
    let mut lines = bytes[..complete].split(|byte| *byte == b'\n');
    let header = match lines.next() {
        Some(line) if complete > 0 => parse_header(id, line)?,
        _ => return Err(corrupt(id, 1, "缺少完整的文件头")),
    };

    // 完整部分以换行符结尾，切分出的最后一段为空，它的行号恰好是末尾残行的行号
    let mut entries = Vec::new();
    let mut number = 1;
    for line in lines {
        number += 1;
        if line.trim_ascii().is_empty() {
            continue;
        }
        let entry = serde_json::from_slice(line).map_err(|error| corrupt(id, number, error))?;
        entries.push(entry);
    }

    let mut warnings = Vec::new();
    let partial = &bytes[complete..];
    if !partial.is_empty() {
        warnings.push(format!(
            "第 {number} 行不完整（{} 字节，可能是写入时进程中断），已忽略",
            partial.len()
        ));
    }
    Ok(StoredSession {
        header,
        entries,
        warnings,
    })
}

/// 解析文件头：先检查格式版本，不猜测更高版本的语义；文件头中的 ID 必须与文件名一致
pub(super) fn parse_header(id: &SessionId, line: &[u8]) -> Result<SessionHeader, StoreError> {
    let value: Value = serde_json::from_slice(line).map_err(|error| corrupt(id, 1, error))?;
    if let Some(version) = value.get("format_version").and_then(Value::as_u64) {
        if version > u64::from(FORMAT_VERSION) {
            return Err(StoreError::UnsupportedVersion {
                id: id.clone(),
                version: u32::try_from(version).unwrap_or(u32::MAX),
                supported: FORMAT_VERSION,
            });
        }
    }
    let header: SessionHeader =
        serde_json::from_value(value).map_err(|error| corrupt(id, 1, error))?;
    if header.id != *id {
        return Err(corrupt(
            id,
            1,
            format!("文件头中的会话 ID {} 与文件名不符", header.id),
        ));
    }
    Ok(header)
}

fn corrupt(id: &SessionId, line: usize, reason: impl ToString) -> StoreError {
    StoreError::Corrupt {
        id: id.clone(),
        line,
        reason: reason.to_string(),
    }
}
