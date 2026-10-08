//! 将消息文件块中的本地 path 简写转换成标准 FileData，完整校验后才读取文件。

use super::super::ProbeResult;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use zach_ai_core::{CallOptions, FileData};

pub(super) struct LocalFile {
    pointer: String,
    path: PathBuf,
}

pub(super) fn prepare(request: &mut Value) -> ProbeResult<Vec<LocalFile>> {
    let mut files = Vec::new();
    let Some(messages) = request
        .pointer_mut("/prompt/messages")
        .and_then(Value::as_array_mut)
    else {
        return Ok(files);
    };
    for (message_index, message) in messages.iter_mut().enumerate() {
        let role = message.get("role").and_then(Value::as_str);
        let allows_file = matches!(role, Some("user" | "assistant"));
        let allows_reasoning = role == Some("assistant");
        let allows_result = matches!(role, Some("assistant" | "tool"));
        let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        // 仅遍历标准内容位置，不把工具参数、JSON 结果或厂商选项当作附件。
        for (part_index, part) in content.iter_mut().enumerate() {
            let pointer = format!("/prompt/messages/{message_index}/content/{part_index}");
            match part.get("type").and_then(Value::as_str) {
                Some("file") if allows_file => prepare_file(part, &pointer, &mut files)?,
                Some("reasoning_file") if allows_reasoning => {
                    prepare_file(part, &pointer, &mut files)?;
                }
                Some("tool_result") if allows_result => {
                    reject_misplaced_path(part)?;
                    prepare_result(part, &pointer, &mut files)?;
                }
                _ => reject_misplaced_path(part)?,
            }
        }
    }
    Ok(files)
}

fn prepare_result(part: &mut Value, pointer: &str, files: &mut Vec<LocalFile>) -> ProbeResult<()> {
    let Some(output) = part.get_mut("output") else {
        return Ok(());
    };
    if output.get("type").and_then(Value::as_str) != Some("content") {
        return Ok(());
    }
    if let Some(content) = output.get_mut("value").and_then(Value::as_array_mut) {
        for (index, block) in content.iter_mut().enumerate() {
            if block.get("type").and_then(Value::as_str) == Some("file") {
                prepare_file(block, &format!("{pointer}/output/value/{index}"), files)?;
            } else {
                reject_misplaced_path(block)?;
            }
        }
    }
    Ok(())
}

fn reject_misplaced_path(part: &Value) -> ProbeResult<()> {
    if part.get("path").is_some() {
        return Err("path 简写只能用于消息中支持的文件块".into());
    }
    Ok(())
}

fn prepare_file(part: &mut Value, pointer: &str, files: &mut Vec<LocalFile>) -> ProbeResult<()> {
    let Some(path) = part.get("path") else {
        return Ok(());
    };
    if part.get("data").is_some() {
        return Err("同一文件不能同时提供 data 和 path；本地附件请省略 data 字段".into());
    }
    let path = path
        .as_str()
        .filter(|path| !path.trim().is_empty())
        .ok_or("文件块的 path 必须是非空字符串")?;
    if part
        .get("media_type")
        .and_then(Value::as_str)
        .is_none_or(|media_type| media_type.trim().is_empty())
    {
        return Err("本地文件块必须提供非空 media_type".into());
    }
    files.push(LocalFile {
        pointer: format!("{pointer}/data"),
        path: PathBuf::from(path),
    });
    let part = part.as_object_mut().expect("已识别文件块对象");
    part.remove("path");
    // 临时数据只用于读取文件前的标准类型校验，不要求场景作者提供占位符。
    part.insert("data".into(), json!({"type":"data", "data":""}));
    Ok(())
}

pub(super) fn bind(
    request: &mut CallOptions,
    files: &[LocalFile],
    scenario_path: &Path,
    mut read: impl FnMut(&Path) -> ProbeResult<Vec<u8>>,
) -> ProbeResult<()> {
    if files.is_empty() {
        return Ok(());
    }
    let mut value = serde_json::to_value(&*request).map_err(|_| "序列化标准请求失败")?;
    for file in files {
        let path = scenario_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&file.path);
        let bytes = read(&path).map_err(|e| format!("读取附件 {} 失败: {e}", path.display()))?;
        if bytes.is_empty() {
            return Err(format!("附件 {} 为空", path.display()));
        }
        *value.pointer_mut(&file.pointer).ok_or("未找到文件块")? =
            serde_json::to_value(FileData::from_bytes(bytes)).map_err(|_| "编码附件失败")?;
    }
    *request = serde_json::from_value(value).map_err(|_| "加载附件后的标准请求无效")?;
    Ok(())
}
