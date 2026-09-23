//! 非流式完整生成结果定义

use crate::options::{ModelWarning, ProviderMetadata};
use crate::prompt::message::Message;
use crate::prompt::part::AssistantPart;
use crate::response::content::OutputContent;
use crate::response::usage::Usage;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 统一归一化的模型结束原因
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnifiedFinishReason {
    /// 正常完成（遇到天然结束符或自定义停词）
    Stop,
    /// 达到最大 Token 上限截断
    Length,
    /// 触发安全/内容审查策略过滤
    ContentFilter,
    /// 模型决定调用一个或多个工具
    ToolCalls,
    /// 生成过程中发生错误中断
    Error,
    /// 厂商特有的其他未知原因
    Other,
    /// 流未正常收尾：既没有收到结束事件，也没有报告错误（通常是连接被静默截断）
    Unknown,
}

/// 模型结束原因（包含统一归一化原因与厂商原始原因）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinishReason {
    pub unified: UnifiedFinishReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl FinishReason {
    pub fn stop() -> Self {
        Self {
            unified: UnifiedFinishReason::Stop,
            raw: None,
        }
    }

    pub fn tool_calls() -> Self {
        Self {
            unified: UnifiedFinishReason::ToolCalls,
            raw: None,
        }
    }
}

/// 响应层元数据
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResponseMetadata {
    /// 厂商返回的响应 ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 厂商响应时间戳（毫秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// 厂商实际生效的模型 ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

impl ResponseMetadata {
    /// 按字段合并：`other` 中为 `Some` 的字段覆盖当前值，`None` 不会清空已有字段
    pub fn merge(&mut self, other: ResponseMetadata) {
        if other.id.is_some() {
            self.id = other.id;
        }
        if other.timestamp.is_some() {
            self.timestamp = other.timestamp;
        }
        if other.model_id.is_some() {
            self.model_id = other.model_id;
        }
    }
}

/// 非流式生成结果
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerateResult {
    /// 模型生成的有序内容块列表
    pub content: Vec<OutputContent>,
    /// 结束原因
    pub finish_reason: FinishReason,
    /// Token 消耗统计
    pub usage: Usage,
    /// 调用产生的警告列表
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ModelWarning>,
    /// 厂商专属元数据
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
    /// 响应元数据（ID、时间戳、实际模型等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseMetadata>,
    /// 调试/遥测：发给厂商的原始请求体
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Value>,
    /// 调试/遥测：厂商响应 HTTP 头
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<HashMap<String, String>>,
}

impl GenerateResult {
    /// 拼接提取所有文本块内容
    pub fn text(&self) -> String {
        let mut full = String::new();
        for item in &self.content {
            if let Some(t) = item.as_text() {
                full.push_str(t);
            }
        }
        full
    }

    /// 提取拼接思考过程内容
    pub fn reasoning(&self) -> Option<String> {
        let mut full = String::new();
        let mut has_any = false;
        for item in &self.content {
            if let Some(r) = item.as_reasoning() {
                full.push_str(r);
                has_any = true;
            }
        }
        if has_any {
            Some(full)
        } else {
            None
        }
    }

    /// 将当前的完整输出转换为下一轮可继续追加的 Assistant 消息
    pub fn into_assistant_message(self) -> Message {
        let parts: Vec<AssistantPart> = self
            .content
            .into_iter()
            .filter_map(|c| c.into_assistant_part())
            .collect();
        Message::Assistant {
            content: parts,
            provider_options: self.provider_metadata,
        }
    }
}
