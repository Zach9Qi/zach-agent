//! 大对象外置：落盘前把消息中较大的文件数据换成 blob 引用，交给模型前再还原
//!
//! 引用以 `FileData::Url { url: "zach-blob:sha256-…" }` 表达，消息结构保持不变。

use super::store::{BlobRef, SessionStore, StoreError};
use std::collections::{HashMap, HashSet};
use zach_ai_core::{
    AssistantPart, FileData, Message, ToolPart, ToolResultContentBlock, ToolResultOutput, UserPart,
};

/// blob 引用的 URL 前缀
const SCHEME: &str = "zach-blob:";

/// 把超过 `threshold` 字节的文件数据写入存储，消息中改为引用；blob 先于引用落盘
pub(super) async fn externalize(
    message: &mut Message,
    store: &dyn SessionStore,
    threshold: usize,
) -> Result<(), StoreError> {
    let mut pending = Vec::new();
    visit_files(message, &mut |data| {
        let Some(bytes) = data.as_bytes().filter(|bytes| bytes.len() > threshold) else {
            return;
        };
        let blob = BlobRef::of(bytes);
        pending.push((blob.clone(), bytes.to_vec()));
        *data = FileData::from_url(format!("{SCHEME}{blob}"));
    });
    for (blob, bytes) in pending {
        store.put_blob(&blob, &bytes).await?;
    }
    Ok(())
}

/// 把消息中的 blob 引用还原为文件数据
pub(super) async fn internalize(
    message: &mut Message,
    store: &dyn SessionStore,
) -> Result<(), StoreError> {
    let mut wanted = HashSet::new();
    visit_files(message, &mut |data| wanted.extend(blob_of(data)));
    if wanted.is_empty() {
        return Ok(());
    }
    let mut loaded = HashMap::new();
    for blob in wanted {
        let bytes = store.get_blob(&blob).await?;
        loaded.insert(blob, bytes);
    }
    visit_files(message, &mut |data| {
        if let Some(bytes) = blob_of(data).and_then(|blob| loaded.get(&blob)) {
            *data = FileData::from_bytes(bytes.clone());
        }
    });
    Ok(())
}

fn blob_of(data: &FileData) -> Option<BlobRef> {
    match data {
        FileData::Url { url } => url.strip_prefix(SCHEME).and_then(BlobRef::parse),
        _ => None,
    }
}

/// 访问消息中的每一处文件数据
fn visit_files(message: &mut Message, visit: &mut dyn FnMut(&mut FileData)) {
    match message {
        Message::System { .. } => {}
        Message::User { content, .. } => {
            for part in content {
                if let UserPart::File { data, .. } = part {
                    visit(data);
                }
            }
        }
        Message::Assistant { content, .. } => {
            for part in content {
                match part {
                    AssistantPart::File { data, .. }
                    | AssistantPart::ReasoningFile { data, .. } => visit(data),
                    AssistantPart::ToolResult { output, .. } => visit_output(output, visit),
                    _ => {}
                }
            }
        }
        Message::Tool { content, .. } => {
            for part in content {
                if let ToolPart::ToolResult { output, .. } = part {
                    visit_output(output, visit);
                }
            }
        }
    }
}

fn visit_output(output: &mut ToolResultOutput, visit: &mut dyn FnMut(&mut FileData)) {
    if let ToolResultOutput::Content { value } = output {
        for block in value {
            if let ToolResultContentBlock::File { data, .. } = block {
                visit(data);
            }
        }
    }
}
