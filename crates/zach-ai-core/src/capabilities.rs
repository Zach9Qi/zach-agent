//! 模型适配器对外声明的可观测能力。

/// 模型当前可被调用方依赖的能力集合。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelCapabilities {
    /// 推理相关的可观测能力。
    pub reasoning: ReasoningCapabilities,
}

/// 推理输出的可观测能力。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReasoningCapabilities {
    /// 是否会在用量中提供推理 token 数量。
    pub tokens: bool,
    /// 是否会提供可见的推理摘要正文。
    pub summary: bool,
    /// 是否会通过流事件提供推理增量。
    pub stream: bool,
    /// 是否支持把推理内容回放到后续请求。
    pub replay: bool,
}
