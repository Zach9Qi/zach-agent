//! 会话错误

use super::log::LogError;
use super::store::StoreError;
use zach_agent::{AgentError, RunError};

/// 会话操作失败
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// 会话正在运行
    #[error("会话正在运行，请等待当前运行结束")]
    Busy,
    /// 存储读写失败
    #[error(transparent)]
    Store(#[from] StoreError),
    /// 日志不变量被破坏（如回退目标不存在）
    #[error(transparent)]
    Log(#[from] LogError),
    /// Agent 拒绝启动运行
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// 运行失败
    #[error(transparent)]
    Run(#[from] RunError),
    /// 此前写入失败，会话已停止；条目可能没有全部落盘
    #[error("会话写入失败，已停止: {0}")]
    Persist(String),
    /// 生成压缩摘要失败
    #[error("上下文压缩失败: {0}")]
    Compaction(String),
    /// 运行任务异常退出
    #[error("会话运行任务异常退出: {0}")]
    Internal(String),
}
