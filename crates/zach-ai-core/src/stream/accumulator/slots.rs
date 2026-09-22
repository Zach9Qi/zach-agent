//! 累加器内部槽位维护：文本、推理与工具调用块的定位、拼接与元数据合并

use crate::options::ProviderMetadata;
use crate::response::OutputContent;
use crate::stream::accumulator::StreamAccumulator;

impl StreamAccumulator {
    pub(super) fn ensure_text(&mut self, id: &str, metadata: Option<ProviderMetadata>) -> usize {
        if let Some(&idx) = self.text_index.get(id) {
            if let OutputContent::Text {
                provider_metadata, ..
            } = &mut self.content[idx]
            {
                merge_metadata(provider_metadata, metadata);
            }
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::Text {
                text: String::new(),
                provider_metadata: metadata,
            });
            self.text_index.insert(id.to_string(), idx);
            idx
        }
    }

    pub(super) fn append_text(
        &mut self,
        idx: usize,
        delta: &str,
        metadata: Option<ProviderMetadata>,
    ) {
        if let OutputContent::Text {
            text,
            provider_metadata,
        } = &mut self.content[idx]
        {
            text.push_str(delta);
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn finish_text(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.text_index.get(id) else {
            return;
        };
        if let OutputContent::Text {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn ensure_reasoning(
        &mut self,
        id: &str,
        metadata: Option<ProviderMetadata>,
    ) -> usize {
        if let Some(&idx) = self.reasoning_index.get(id) {
            if let OutputContent::Reasoning {
                provider_metadata, ..
            } = &mut self.content[idx]
            {
                merge_metadata(provider_metadata, metadata);
            }
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::Reasoning {
                text: String::new(),
                provider_metadata: metadata,
            });
            self.reasoning_index.insert(id.to_string(), idx);
            idx
        }
    }

    pub(super) fn append_reasoning(
        &mut self,
        idx: usize,
        delta: &str,
        metadata: Option<ProviderMetadata>,
    ) {
        if let OutputContent::Reasoning {
            text,
            provider_metadata,
        } = &mut self.content[idx]
        {
            text.push_str(delta);
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn finish_reasoning(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.reasoning_index.get(id) else {
            return;
        };
        if let OutputContent::Reasoning {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn begin_tool(
        &mut self,
        id: &str,
        tool_name: String,
        provider_executed: bool,
        dynamic: bool,
        metadata: Option<ProviderMetadata>,
    ) {
        let idx = self.ensure_tool(id);
        let sealed = self.sealed_tools.contains(id);
        if let OutputContent::ToolCall {
            tool_name: name,
            provider_executed: executed,
            dynamic: is_dynamic,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !tool_name.is_empty() && (name.is_empty() || !sealed) {
                *name = tool_name;
            }
            if !sealed {
                *executed = provider_executed;
                *is_dynamic = dynamic;
            }
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn append_tool_input(
        &mut self,
        id: &str,
        delta: &str,
        metadata: Option<ProviderMetadata>,
    ) {
        let idx = self.ensure_tool(id);
        let sealed = self.sealed_tools.contains(id);
        if let OutputContent::ToolCall {
            input,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !sealed {
                input.push_str(delta);
            }
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn finish_tool_input(&mut self, id: &str, metadata: Option<ProviderMetadata>) {
        let Some(&idx) = self.tool_index.get(id) else {
            return;
        };
        if let OutputContent::ToolCall {
            provider_metadata, ..
        } = &mut self.content[idx]
        {
            merge_metadata(provider_metadata, metadata);
        }
    }

    pub(super) fn apply_tool_call(
        &mut self,
        id: String,
        tool_name: String,
        input: String,
        provider_executed: bool,
        dynamic: bool,
        metadata: Option<ProviderMetadata>,
    ) {
        let seal = !input.is_empty();
        let idx = self.ensure_tool(&id);
        if let OutputContent::ToolCall {
            tool_name: name,
            input: slot_input,
            provider_executed: executed,
            dynamic: is_dynamic,
            provider_metadata,
            ..
        } = &mut self.content[idx]
        {
            if !tool_name.is_empty() {
                *name = tool_name;
            }
            if seal {
                *slot_input = input;
            }
            *executed = provider_executed;
            *is_dynamic = dynamic;
            merge_metadata(provider_metadata, metadata);
        }
        if seal {
            self.sealed_tools.insert(id);
        }
    }

    /// 写入工具结果：同一 `tool_call_id` 的后续结果（如中间态进度 → 最终结果）原地替换，
    /// 位置沿用首次出现处，避免 `content` 中残留多份进度快照。
    pub(super) fn apply_tool_result(&mut self, result: OutputContent) {
        let OutputContent::ToolResult { tool_call_id, .. } = &result else {
            return;
        };
        if let Some(&idx) = self.tool_result_index.get(tool_call_id) {
            self.content[idx] = result;
        } else {
            self.tool_result_index
                .insert(tool_call_id.clone(), self.content.len());
            self.content.push(result);
        }
    }

    fn ensure_tool(&mut self, id: &str) -> usize {
        if let Some(&idx) = self.tool_index.get(id) {
            idx
        } else {
            let idx = self.content.len();
            self.content.push(OutputContent::ToolCall {
                tool_call_id: id.to_string(),
                tool_name: String::new(),
                input: String::new(),
                provider_executed: false,
                dynamic: false,
                provider_metadata: None,
            });
            self.tool_index.insert(id.to_string(), idx);
            idx
        }
    }
}

/// 将新到达的元数据并入槽位；`None` 不会清空既有值。
///
/// Start / Delta / End 各阶段可能分别携带不同字段（如 Anthropic 在 End 才给出 `signature`），
/// 因此按厂商键合并而不是整体覆盖。
pub(super) fn merge_metadata(
    slot: &mut Option<ProviderMetadata>,
    incoming: Option<ProviderMetadata>,
) {
    let Some(incoming) = incoming else {
        return;
    };
    match slot {
        Some(existing) => existing.merge(incoming),
        None => *slot = Some(incoming),
    }
}
