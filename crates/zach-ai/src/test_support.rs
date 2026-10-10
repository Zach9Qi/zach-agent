//! 单元测试共用夹具：手工构造的模型档案，以及流式测试的字节源、SSE 帧与事件聚合。
//!
//! 适配器测试用这里手工构造的档案，而不是读取随 models.dev 同步的内置目录：目录数据一变
//! （某个模型新增档位、改为不接受 temperature），测适配器逻辑的用例不应随之失败。

use bytes::Bytes;
use futures::Stream;
use serde_json::Value;
use zach_ai_core::{
    GenerateResult, ModelError, ModelProfile, ReasoningEffort, ReasoningProfile, StreamAccumulator,
    StreamPart,
};

/// 手工构造档案：`reasoning` 为 `None` 表示不支持推理，`temperature` 为假表示模型拒绝该参数。
pub(crate) fn profile(
    provider: &str,
    id: &str,
    reasoning: Option<ReasoningProfile>,
    temperature: bool,
) -> ModelProfile {
    let mut profile = ModelProfile::new(provider, id, 200_000, 64_000);
    profile.reasoning = reasoning;
    profile.temperature = temperature;
    profile
}

/// 推理能力：`efforts` 为空表示档位未知、不做限制。
pub(crate) fn reasoning_profile(
    efforts: &[ReasoningEffort],
    can_disable: bool,
) -> Option<ReasoningProfile> {
    Some(ReasoningProfile {
        efforts: efforts.to_vec(),
        can_disable,
    })
}

/// 把整段线上文本按单字节切分为字节源，确保 UTF-8 字符与 SSE 帧边界都会跨越传输块。
pub(crate) fn byte_stream(
    wire: String,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures::stream::iter(
        wire.into_bytes()
            .into_iter()
            .map(|byte| Ok(Bytes::from(vec![byte]))),
    )
}

/// 带 `event:` 行的 SSE 帧（Anthropic Messages 与 OpenAI Responses 的形态），行结束符用 CRLF
/// 以覆盖解码器的另一条分支。
pub(crate) fn event_frame(value: &Value) -> String {
    let kind = value["type"].as_str().expect("事件带 type 字段");
    format!(
        "event: {kind}
data: {value}

"
    )
}

/// 只有 `data:` 行的 SSE 帧（Chat Completions 的形态）。
pub(crate) fn data_frame(value: &Value) -> String {
    format!(
        "data: {value}

"
    )
}

/// 全部事件必须成功、流已收尾且没有错误事件，聚合为最终结果。
pub(crate) fn aggregate(parts: Vec<Result<StreamPart, ModelError>>) -> GenerateResult {
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    assert!(accumulator.is_complete());
    assert!(
        accumulator.errors().is_empty(),
        "{:?}",
        accumulator.errors()
    );
    accumulator.finish()
}

/// 不校验收尾与错误事件的聚合，用于观察失败路径的最终结束原因与已收集的诊断内容。
pub(crate) fn aggregate_lenient(parts: Vec<Result<StreamPart, ModelError>>) -> GenerateResult {
    let mut accumulator = StreamAccumulator::new();
    for part in parts {
        accumulator.process(part.unwrap());
    }
    accumulator.finish()
}
