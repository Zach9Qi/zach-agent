//! 按历史顺序回放 Messages 消息：系统提示提升为 `system`，同角色相邻消息合并。

use serde_json::{json, Value};
use zach_ai_core::{Message, ModelError};

use super::{blocks, cache_control, with_cache_control};

/// 转换结果：顶层 `system` 块与交替的 `messages`。
pub(super) struct Converted {
    pub(super) system: Vec<Value>,
    pub(super) messages: Vec<Value>,
}

pub(super) fn convert(messages: &[Message]) -> Result<Converted, ModelError> {
    let mut converted = Converted {
        system: Vec::new(),
        messages: Vec::new(),
    };
    for message in messages {
        match message {
            Message::System {
                content,
                provider_options,
            } => {
                if !converted.messages.is_empty() {
                    return Err(ModelError::InvalidRequest(
                        "Messages API 的系统提示只能出现在对话开头".into(),
                    ));
                }
                converted.system.push(with_cache_control(
                    json!({"type": "text", "text": content}),
                    cache_control(provider_options.as_ref()),
                ));
            }
            Message::User { content, .. } => {
                let blocks = content
                    .iter()
                    .map(blocks::user_part)
                    .collect::<Result<Vec<_>, _>>()?;
                push(&mut converted.messages, "user", blocks);
            }
            Message::Assistant { content, .. } => {
                let mut blocks = Vec::new();
                for part in content {
                    if let Some(block) = blocks::assistant_part(part)? {
                        blocks.push(block);
                    }
                }
                // 没有任何可回放块（如仅含无签名的外部推理）的助手消息不发送空内容。
                if !blocks.is_empty() {
                    push(&mut converted.messages, "assistant", blocks);
                }
            }
            Message::Tool { content, .. } => {
                let blocks = content
                    .iter()
                    .map(blocks::tool_part)
                    .collect::<Result<Vec<_>, _>>()?;
                push(&mut converted.messages, "user", blocks);
            }
        }
    }
    trim_trailing_assistant_text(&mut converted.messages);
    Ok(converted)
}

/// 追加一条消息；与上一条角色相同时合并内容块，保持 user/assistant 交替。
fn push(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            if let Some(content) = last["content"].as_array_mut() {
                content.extend(blocks);
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": blocks}));
}

/// Messages API 拒绝以空白结尾的助手预填充文本，回放时去掉末尾空白。
fn trim_trailing_assistant_text(messages: &mut [Value]) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    if last["role"] != "assistant" {
        return;
    }
    let Some(block) = last["content"]
        .as_array_mut()
        .and_then(|content| content.last_mut())
    else {
        return;
    };
    if block["type"] == "text" {
        if let Some(text) = block["text"].as_str() {
            let trimmed = text.trim_end().to_owned();
            block["text"] = Value::String(trimmed);
        }
    }
}
