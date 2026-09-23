//! 循环配置、重试策略与消息队列模式

use crate::hooks::{AgentHooks, NoopHooks};
use crate::tool::ToolExecutionMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use zach_ai_core::{CallOptions, LanguageModel};

/// 插队与追加消息在注入点的取用方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// 一次取出全部排队消息
    All,
    /// 每个注入点只取最早的一条
    #[default]
    OneAtATime,
}

/// 模型请求失败时的重试策略（指数退避）
///
/// 只对 [`zach_ai_core::ModelError::is_retryable`] 为真的错误生效。
#[derive(Debug, Clone, PartialEq)]
pub struct RetryPolicy {
    /// 最大重试次数（不含首次请求）
    pub max_retries: u32,
    /// 首次重试前的等待时长
    pub initial_delay: Duration,
    /// 单次等待上限
    pub max_delay: Duration,
    /// 每次重试等待时长的放大倍数
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    /// 不重试
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Self::default()
        }
    }

    /// 第 `attempt` 次重试（从 1 开始）前的等待时长
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1) as i32;
        let secs = self.initial_delay.as_secs_f64() * self.multiplier.powi(exponent);
        Duration::from_secs_f64(secs.min(self.max_delay.as_secs_f64()))
    }
}

/// 低层循环配置
#[derive(Clone)]
pub struct LoopConfig {
    /// 初始模型，可被钩子逐轮替换
    pub model: Arc<dyn LanguageModel>,
    /// 调用参数模板（采样参数、推理强度、厂商选项等）
    pub options: CallOptions,
    /// 策略钩子
    pub hooks: Arc<dyn AgentHooks>,
    /// 工具批次执行方式
    pub tool_execution: ToolExecutionMode,
    /// 模型请求重试策略
    pub retry: RetryPolicy,
}

impl LoopConfig {
    /// 以默认参数构造
    pub fn new(model: Arc<dyn LanguageModel>) -> Self {
        Self {
            model,
            options: CallOptions::default(),
            hooks: Arc::new(NoopHooks),
            tool_execution: ToolExecutionMode::default(),
            retry: RetryPolicy::default(),
        }
    }

    /// 设置调用参数模板
    pub fn with_options(mut self, options: CallOptions) -> Self {
        self.options = options;
        self
    }

    /// 设置钩子
    pub fn with_hooks(mut self, hooks: Arc<dyn AgentHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// 设置工具执行方式
    pub fn with_tool_execution(mut self, mode: ToolExecutionMode) -> Self {
        self.tool_execution = mode;
        self
    }

    /// 设置重试策略
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}
