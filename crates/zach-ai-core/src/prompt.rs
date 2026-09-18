//! 提示词（Prompt）与消息模型入口

pub mod message;
pub mod part;

pub use message::{Message, Prompt};
pub use part::{AssistantPart, ToolPart, UserPart};
