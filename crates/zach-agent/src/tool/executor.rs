//! 工具执行抽象与适配器

use std::future::Future;
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;
use zach_ai_core::ToolResultOutput;
use crate::error::AgentError;

/// 异步工具执行器抽象
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// 执行工具调用并返回标准输出载荷
    async fn execute(&self, input: Value) -> Result<ToolResultOutput, AgentError>;
}

#[async_trait]
impl<T: ?Sized + ToolExecutor> ToolExecutor for Box<T> {
    async fn execute(&self, input: Value) -> Result<ToolResultOutput, AgentError> {
        (**self).execute(input).await
    }
}

#[async_trait]
impl<T: ?Sized + ToolExecutor> ToolExecutor for Arc<T> {
    async fn execute(&self, input: Value) -> Result<ToolResultOutput, AgentError> {
        (**self).execute(input).await
    }
}

/// 基于异步闭包的便捷工具执行器
pub struct AsyncFnExecutor<F> {
    handler: F,
}

impl<F> AsyncFnExecutor<F> {
    /// 构造新的异步闭包执行器
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl<F, Fut> ToolExecutor for AsyncFnExecutor<F>
where
    F: Fn(Value) -> Fut + Send + Sync,
    Fut: Future<Output = Result<ToolResultOutput, AgentError>> + Send + 'static,
{
    async fn execute(&self, input: Value) -> Result<ToolResultOutput, AgentError> {
        (self.handler)(input).await
    }
}

/// 基于同步闭包的便捷工具执行器
pub struct SyncFnExecutor<F> {
    handler: F,
}

impl<F> SyncFnExecutor<F> {
    /// 构造新的同步闭包执行器
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

#[async_trait]
impl<F> ToolExecutor for SyncFnExecutor<F>
where
    F: Fn(Value) -> Result<ToolResultOutput, AgentError> + Send + Sync,
{
    async fn execute(&self, input: Value) -> Result<ToolResultOutput, AgentError> {
        (self.handler)(input)
    }
}
