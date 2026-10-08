//! 文本与推理分块维护：补齐最终快照，并防止重复追加正文。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ProviderMetadata, StreamPart};

use super::super::response::metadata;
use super::parser::ResponsesStreamParser;

pub(super) struct Block {
    text: String,
    ended: bool,
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

    pub(super) fn reconcile(
        &mut self,
        id: &str,
        full: &str,
        reasoning: bool,
        metadata: Option<ProviderMetadata>,
        parts: &mut Vec<StreamPart>,
    ) -> Result<(), ModelError> {
        self.start_block(id, reasoning, metadata, parts);
        let block = &self.blocks[id];
        let suffix = full
            .strip_prefix(&block.text)
            .ok_or_else(|| {
                ModelError::provider_error("openai", "最终文本与已收到的增量不一致", None)
            })?
            .to_owned();
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
