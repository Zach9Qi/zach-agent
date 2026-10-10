//! 单元测试共用夹具。
//!
//! 适配器测试用这里手工构造的档案，而不是读取随 models.dev 同步的内置目录：目录数据一变
//! （某个模型新增档位、改为不接受 temperature），测适配器逻辑的用例不应随之失败。

use zach_ai_core::{ModelProfile, ReasoningEffort, ReasoningProfile};

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
