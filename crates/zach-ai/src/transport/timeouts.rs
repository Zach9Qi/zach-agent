//! 适配器的超时策略：非流式整体超时与流式空闲超时。
//!
//! 两类请求的时间特征不同：非流式请求在服务端生成完之前收不到任何字节，只能按整体时长
//! 设限；流式请求的总时长由生成长度决定，只能约束"多久没有新数据"。reqwest 客户端级的
//! `read_timeout` 对两者一视同仁，会把长时间推理的非流式请求误杀，因此按请求类型分别处理。

use std::time::Duration;

/// 非流式请求的默认整体超时，与官方 SDK 的默认值一致。
const DEFAULT_GENERATE: Duration = Duration::from_secs(600);

/// 流式请求相邻两次收到数据之间的默认上限：推理模型可能长时间不吐字，但不应无限等待。
const DEFAULT_STREAM_IDLE: Duration = Duration::from_secs(300);

/// 一个模型实例的超时配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timeouts {
    /// 非流式请求从发起连接到读完响应体的上限；`None` 不限制。
    pub(crate) generate: Option<Duration>,
    /// 流式请求等待响应头、以及相邻两次收到数据之间的上限；`None` 不限制。
    pub(crate) stream_idle: Option<Duration>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            generate: Some(DEFAULT_GENERATE),
            stream_idle: Some(DEFAULT_STREAM_IDLE),
        }
    }
}

impl Timeouts {
    /// 发送阶段（等待响应头、读取错误正文）的守卫时长。
    ///
    /// 非流式请求已由请求级整体超时覆盖，这里只为流式请求提供空闲守卫。
    pub(crate) fn guard(&self, stream: bool) -> Option<Duration> {
        if stream {
            self.stream_idle
        } else {
            None
        }
    }
}
