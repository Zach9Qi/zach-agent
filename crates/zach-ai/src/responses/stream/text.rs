//! 文本与推理分块维护：补齐最终快照，并防止重复追加正文。
//!
//! `*.done` 事件携带的完整快照通常是已收增量的延长；中间代理改写文本（如去掉首尾空白）时两者
//! 会对不上。已发出的增量无法撤回，因此不把这种不一致判为失败：保留流式正文，把最终快照放进
//! 该块 End 事件的元数据（`openai.final_snapshot`）供调用方自行取舍。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ProviderMetadata, StreamPart};

use super::super::response::metadata;
use super::parser::ResponsesStreamParser;

pub(super) struct Block {
    text: String,
    ended: bool,
    /// `done` 快照与已收增量不一致时记下的最终文本，随 End 元数据透出。
    final_snapshot: Option<String>,
}

impl ResponsesStreamParser {
    pub(super) fn text_metadata(&self, event: &Value, refusal: bool) -> Option<ProviderMetadata> {
        let id = event["item_id"].as_str().unwrap_or_default();
        metadata(json!({
            "item_id": id, "refusal": refusal,
            "phase": self.items.get(id).and_then(|item| item.get("phase")),
        }))
    }

    pub(super) fn start_block(
        &mut self,
        id: &str,
        reasoning: bool,
        metadata: Option<ProviderMetadata>,
        parts: &mut Vec<StreamPart>,
    ) {
        if self.blocks.contains_key(id) {
            return;
        }
        self.blocks.insert(
            id.into(),
            Block {
                text: String::new(),
                ended: false,
                final_snapshot: None,
            },
        );
        parts.push(if reasoning {
            StreamPart::ReasoningStart {
                id: id.into(),
                provider_metadata: metadata,
            }
        } else {
            StreamPart::TextStart {
                id: id.into(),
                provider_metadata: metadata,
            }
        });
    }

    pub(super) fn delta(
        &mut self,
        id: &str,
        text: &str,
        reasoning: bool,
        metadata: Option<ProviderMetadata>,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        self.start_block(id, reasoning, metadata, parts);
        let block = self.blocks.get_mut(id).expect("分块已建立");
        if block.ended {
            return Err(ModelError::provider_error(
                "openai",
                "已结束的分块又收到增量",
                None,
            ));
        }
        block.text.push_str(text);
        if !text.is_empty() {
            parts.push(if reasoning {
                StreamPart::ReasoningDelta {
                    id: id.into(),
                    delta: text.into(),
                    provider_metadata: None,
                }
            } else {
                StreamPart::TextDelta {
                    id: id.into(),
                    delta: text.into(),
                    provider_metadata: None,
                }
            });
        }
        Ok(())
    }

    /// 用完成快照补齐分块：快照是已收增量的延长时发出差额；对不上时记录快照而不判失败。
    pub(super) fn reconcile(
        &mut self,
        id: &str,
        full: &str,
        reasoning: bool,
        metadata: Option<ProviderMetadata>,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        self.start_block(id, reasoning, metadata, parts);
        let block = self.blocks.get_mut(id).expect("分块已建立");
        let Some(suffix) = full.strip_prefix(block.text.as_str()).map(str::to_owned) else {
            block.final_snapshot = Some(full.to_owned());
            return Ok(());
        };
        if !suffix.is_empty() {
            self.delta(id, &suffix, reasoning, None, parts)?;
        }
        Ok(())
    }

    pub(super) fn finish_block(
        &mut self,
        id: &str,
        full: &str,
        reasoning: bool,
        metadata: Option<ProviderMetadata>,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        self.reconcile(id, full, reasoning, metadata.clone(), parts)?;
        let block = self.blocks.get_mut(id).expect("分块已建立");
        if !block.ended {
            block.ended = true;
            let metadata = with_final_snapshot(metadata, block.final_snapshot.take());
            parts.push(if reasoning {
                StreamPart::ReasoningEnd {
                    id: id.into(),
                    provider_metadata: metadata,
                }
            } else {
                StreamPart::TextEnd {
                    id: id.into(),
                    provider_metadata: metadata,
                }
            });
        }
        Ok(())
    }
}

/// 把不一致的最终快照并入 End 事件的 `openai` 元数据。
fn with_final_snapshot(
    metadata: Option<ProviderMetadata>,
    snapshot: Option<String>,
) -> Option<ProviderMetadata> {
    let Some(snapshot) = snapshot else {
        return metadata;
    };
    let mut extra = ProviderMetadata::new();
    extra.insert("openai", json!({"final_snapshot": snapshot}));
    match metadata {
        Some(mut existing) => {
            existing.merge(extra);
            Some(existing)
        }
        None => Some(extra),
    }
}
