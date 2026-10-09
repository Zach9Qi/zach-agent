//! 协议到模型实现的唯一创建入口，公共联调流程不依赖具体厂商。

use super::{
    config::{Config, Protocol},
    ProbeResult,
};
use std::{sync::Arc, time::Duration};
use zach_ai::{AnthropicMessagesModel, OpenAiChatCompletionsModel, OpenAiResponsesModel};
use zach_ai_core::LanguageModel;

pub(super) fn create(config: &Config, timeout: Duration) -> ProbeResult<Arc<dyn LanguageModel>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {e}"))?;
    match config.protocol {
        Protocol::OpenAiResponses => Ok(Arc::new(
            OpenAiResponsesModel::with_client(client, &config.api_key, &config.model)
                .with_base_url(&config.base_url),
        )),
        Protocol::OpenAiChat => Ok(Arc::new(
            OpenAiChatCompletionsModel::with_client(client, &config.api_key, &config.model)
                .with_base_url(&config.base_url),
        )),
        Protocol::AnthropicMessages => Ok(Arc::new(
            AnthropicMessagesModel::with_client(client, &config.api_key, &config.model)
                .with_base_url(&config.base_url),
        )),
    }
}
