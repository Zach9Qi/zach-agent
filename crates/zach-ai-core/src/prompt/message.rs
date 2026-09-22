//! 对话消息结构定义与构造器

use crate::file::FileData;
use crate::options::ProviderOptions;
use crate::prompt::part::{AssistantPart, ToolPart, UserPart};
use serde::{Deserialize, Serialize};

/// 单条对话消息
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// 系统提示词（保持纯文本，契合绝大部分厂商 API）
    System {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 用户消息（包含文本和多模态文件块）
    User {
        content: Vec<UserPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 模型/助手生成消息（文本、思考链、工具调用）
    Assistant {
        content: Vec<AssistantPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// 工具执行结果消息（工具执行响应与审批）
    Tool {
        content: Vec<ToolPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl Message {
    /// 构造纯文本系统消息
    pub fn system(text: impl Into<String>) -> Self {
        Self::System {
            content: text.into(),
            provider_options: None,
        }
    }

    /// 构造纯文本用户消息
    pub fn user(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![UserPart::text(text)],
            provider_options: None,
        }
    }

    /// 构造带多模态文件的用户消息
    pub fn user_with_file(
        text: impl Into<String>,
        media_type: impl Into<String>,
        data: FileData,
    ) -> Self {
        Self::User {
            content: vec![UserPart::text(text), UserPart::file(media_type, data)],
            provider_options: None,
        }
    }

    /// 构造纯文本助手消息
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::Assistant {
            content: vec![AssistantPart::text(text)],
            provider_options: None,
        }
    }

    /// 构造包含工具调用的助手消息
    pub fn assistant_tool_calls(parts: Vec<AssistantPart>) -> Self {
        Self::Assistant {
            content: parts,
            provider_options: None,
        }
    }

    /// 构造工具返回消息
    pub fn tool(parts: Vec<ToolPart>) -> Self {
        Self::Tool {
            content: parts,
            provider_options: None,
        }
    }
}

/// 标准化提示词历史
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    /// 消息列表
    pub messages: Vec<Message>,
}

impl Prompt {
    /// 构造空 Prompt
    pub fn new() -> Self {
        Self::default()
    }

    /// 从消息列表构造
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    /// 追加消息
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// 链式追加消息
    pub fn with_message(mut self, message: Message) -> Self {
        self.messages.push(message);
        self
    }

    /// 链式追加系统提示词
    pub fn with_system(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::system(text));
        self
    }

    /// 链式追加用户提示词
    pub fn with_user(mut self, text: impl Into<String>) -> Self {
        self.messages.push(Message::user(text));
        self
    }

    /// 获取消息数量
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 检查是否为空
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl From<Vec<Message>> for Prompt {
    fn from(messages: Vec<Message>) -> Self {
        Self { messages }
    }
}
