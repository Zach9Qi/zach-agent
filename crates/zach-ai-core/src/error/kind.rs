//! 错误类别

use serde::{Deserialize, Serialize};
use std::fmt;

/// 错误类别：按"调用方该怎么处理"划分，而不是按来源或 HTTP 状态码
///
/// 例如 OpenAI 的 `insufficient_quota` 状态码是 429，但需要充值、重试无济于事，
/// 因此归为 [`Self::Rejected`] 而不是 [`Self::RateLimited`]。
///
/// 序列化值（`snake_case`）是对外的稳定契约。刻意不设"其他"类别：每个出错点都必须做出分类，
/// 实在无法判断时归为 [`Self::Internal`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorKind {
    // —— 修改请求才能成功 ——
    /// 本地校验失败，请求没有发出
    InvalidRequest,
    /// 模型或适配器不支持某项特性
    Unsupported,
    /// 凭据无效或权限不足
    Authentication,
    /// 超出模型上下文窗口；压缩历史后可重试
    ContextLengthExceeded,
    /// 服务端拒绝了请求本身（其余 4xx、配额耗尽、无法识别的厂商错误等）
    Rejected,

    // —— 原样重试可能成功 ——
    /// 触发限流
    RateLimited,
    /// 服务端暂时不可用（5xx、408、过载）
    Unavailable,
    /// 连接失败、读取中断、空闲超时、流在终止事件前结束
    Transport,

    // —— 缺陷 ——
    /// 响应无法解析或违反协议（适配器或网关的问题）
    InvalidResponse,
    /// 本库、钩子或宿主的内部缺陷（panic 等）
    Internal,
}

impl ErrorKind {
    /// 原样重试是否可能成功；可重试性只在这里定义
    ///
    /// 只判断错误性质：流中途失败时可能已产出部分内容，是否真正重试由上层决定。
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Unavailable | Self::Transport
        )
    }

    /// 面向人的简短描述
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidRequest => "请求参数无效",
            Self::Unsupported => "不支持的特性",
            Self::Authentication => "鉴权失败",
            Self::ContextLengthExceeded => "超出上下文窗口",
            Self::Rejected => "请求被拒绝",
            Self::RateLimited => "触发限流",
            Self::Unavailable => "服务暂不可用",
            Self::Transport => "传输失败",
            Self::InvalidResponse => "响应无法解析",
            Self::Internal => "内部错误",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
