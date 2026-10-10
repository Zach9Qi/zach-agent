//! 按模型档案校验通用参数：无法满足的设置要么在发送前报错，要么降级并以警告透出。
//!
//! 档案未知（自定义端点、目录未收录）时全部跳过，参数原样发送，由服务端裁决。
//! 档案已知时的规则：
//! - 模型不支持推理却设置了档位：丢弃并给出 `Unsupported` 警告（`None` 等价于默认，静默忽略）；
//! - 档案未声明可关闭推理却要求 `None`：不发送关闭指令并给出 `Compatibility` 警告；
//! - 档位不在模型支持列表里：适配器提供了降级别名则降级并给出 `Compatibility` 警告，
//!   否则报 `UnsupportedFeature`，因为没有不改变语义的替代方案；
//! - 档案声明不接受 `temperature`：丢弃并给出 `Unsupported` 警告。
//!
//! 警告随 `StreamStart` / `GenerateResult.warnings` 透出，Agent 层会把它们作为事件呈现。

use zach_ai_core::{CallOptions, ModelError, ModelProfile, ModelWarning, ReasoningEffort};

/// 经档案校验后的有效通用参数与产生的警告。
pub(crate) struct Validated {
    pub(crate) reasoning: Option<ReasoningEffort>,
    pub(crate) temperature: Option<f32>,
    pub(crate) warnings: Vec<ModelWarning>,
}

/// 校验 `options` 中的推理档位与温度。
///
/// `aliases` 是适配器定义的降级对：`(请求档位, 替代档位)`，仅在替代档位受支持时生效。
pub(crate) fn validate(
    profile: Option<&ModelProfile>,
    options: &CallOptions,
    aliases: &[(ReasoningEffort, ReasoningEffort)],
) -> Result<Validated, ModelError> {
    let mut validated = Validated {
        reasoning: options.reasoning,
        temperature: options.temperature,
        warnings: Vec::new(),
    };
    let Some(profile) = profile else {
        return Ok(validated);
    };
    validated.reasoning = reasoning(profile, options.reasoning, aliases, &mut validated.warnings)?;
    if validated.temperature.is_some() && !profile.temperature {
        validated.temperature = None;
        validated.warnings.push(ModelWarning::Unsupported {
            feature: "temperature".into(),
            details: Some(format!("模型 {} 不接受 temperature，已忽略", profile.id)),
        });
    }
    Ok(validated)
}

fn reasoning(
    profile: &ModelProfile,
    effort: Option<ReasoningEffort>,
    aliases: &[(ReasoningEffort, ReasoningEffort)],
    warnings: &mut Vec<ModelWarning>,
) -> Result<Option<ReasoningEffort>, ModelError> {
    let Some(effort) = effort else {
        return Ok(None);
    };
    if effort == ReasoningEffort::ProviderDefault {
        return Ok(Some(effort));
    }
    let Some(reasoning) = &profile.reasoning else {
        if effort != ReasoningEffort::None {
            warnings.push(ModelWarning::Unsupported {
                feature: "reasoning".into(),
                details: Some(format!(
                    "模型 {} 不支持推理，已忽略档位 {}",
                    profile.id,
                    effort_name(effort)
                )),
            });
        }
        return Ok(None);
    };
    if effort == ReasoningEffort::None {
        if reasoning.can_disable {
            return Ok(Some(effort));
        }
        warnings.push(ModelWarning::Compatibility {
            feature: "reasoning_effort.none".into(),
            details: Some(format!(
                "模型 {} 的档案未声明可关闭推理，不发送关闭指令，按模型默认行为运行",
                profile.id
            )),
        });
        return Ok(None);
    }
    if reasoning.supports(effort) {
        return Ok(Some(effort));
    }
    if let Some((_, fallback)) = aliases
        .iter()
        .find(|(from, to)| *from == effort && reasoning.supports(*to))
    {
        warnings.push(ModelWarning::Compatibility {
            feature: format!("reasoning_effort.{}", effort_name(effort)),
            details: Some(format!(
                "模型 {} 不支持该档位，已降级为 {}",
                profile.id,
                effort_name(*fallback)
            )),
        });
        return Ok(Some(*fallback));
    }
    let supported: Vec<&str> = reasoning.efforts.iter().copied().map(effort_name).collect();
    Err(ModelError::unsupported(
        format!("reasoning_effort.{}", effort_name(effort)),
        Some(format!(
            "模型 {} 支持的推理档位: {}",
            profile.id,
            supported.join(", ")
        )),
    ))
}

/// 与 `ReasoningEffort` 的 serde 名称一致。
pub(crate) fn effort_name(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::ProviderDefault => "provider_default",
        ReasoningEffort::None => "none",
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        ReasoningEffort::Max => "max",
    }
}

#[cfg(test)]
mod tests {
    //! 档案已知与未知时推理档位、温度的校验结果与警告。
    use super::*;
    use zach_ai_core::ReasoningProfile;

    fn profile(reasoning: Option<ReasoningProfile>, temperature: bool) -> ModelProfile {
        let mut profile = ModelProfile::new("p", "m", 1000, 100);
        profile.reasoning = reasoning;
        profile.temperature = temperature;
        profile
    }

    fn options(effort: Option<ReasoningEffort>, temperature: Option<f32>) -> CallOptions {
        CallOptions {
            reasoning: effort,
            temperature,
            ..Default::default()
        }
    }

    #[test]
    fn unknown_profile_passes_everything_through() {
        let validated =
            validate(None, &options(Some(ReasoningEffort::Max), Some(0.5)), &[]).unwrap();
        assert_eq!(validated.reasoning, Some(ReasoningEffort::Max));
        assert_eq!(validated.temperature, Some(0.5));
        assert!(validated.warnings.is_empty());
    }

    #[test]
    fn non_reasoning_model_drops_effort_with_a_warning_except_none() {
        let profile = profile(None, true);
        let validated = validate(
            Some(&profile),
            &options(Some(ReasoningEffort::High), None),
            &[],
        )
        .unwrap();
        assert_eq!(validated.reasoning, None);
        assert!(matches!(
            &validated.warnings[0],
            ModelWarning::Unsupported { feature, .. } if feature == "reasoning"
        ));
        let silent = validate(
            Some(&profile),
            &options(Some(ReasoningEffort::None), None),
            &[],
        )
        .unwrap();
        assert_eq!(silent.reasoning, None);
        assert!(silent.warnings.is_empty());
        let default = validate(
            Some(&profile),
            &options(Some(ReasoningEffort::ProviderDefault), None),
            &[],
        )
        .unwrap();
        assert_eq!(default.reasoning, Some(ReasoningEffort::ProviderDefault));
    }

    #[test]
    fn effort_outside_the_supported_list_errors_unless_an_alias_applies() {
        let profile = profile(
            Some(ReasoningProfile {
                efforts: vec![ReasoningEffort::Low, ReasoningEffort::High],
                can_disable: false,
            }),
            true,
        );
        assert!(matches!(
            validate(Some(&profile), &options(Some(ReasoningEffort::Medium), None), &[]),
            Err(ModelError::UnsupportedFeature { feature, details: Some(details) })
                if feature == "reasoning_effort.medium" && details.contains("low, high")
        ));
        let aliased = validate(
            Some(&profile),
            &options(Some(ReasoningEffort::Minimal), None),
            &[(ReasoningEffort::Minimal, ReasoningEffort::Low)],
        )
        .unwrap();
        assert_eq!(aliased.reasoning, Some(ReasoningEffort::Low));
        assert!(matches!(
            &aliased.warnings[0],
            ModelWarning::Compatibility { feature, .. } if feature == "reasoning_effort.minimal"
        ));
        // 别名目标本身不受支持时不生效。
        assert!(validate(
            Some(&profile),
            &options(Some(ReasoningEffort::Minimal), None),
            &[(ReasoningEffort::Minimal, ReasoningEffort::Medium)],
        )
        .is_err());
        let exact = validate(
            Some(&profile),
            &options(Some(ReasoningEffort::High), None),
            &[],
        )
        .unwrap();
        assert_eq!(exact.reasoning, Some(ReasoningEffort::High));
        assert!(exact.warnings.is_empty());
    }

    #[test]
    fn disabling_reasoning_on_an_always_on_model_falls_back_to_default() {
        let locked = profile(
            Some(ReasoningProfile {
                efforts: vec![],
                can_disable: false,
            }),
            true,
        );
        let validated = validate(
            Some(&locked),
            &options(Some(ReasoningEffort::None), None),
            &[],
        )
        .unwrap();
        assert_eq!(validated.reasoning, None);
        assert!(matches!(
            &validated.warnings[0],
            ModelWarning::Compatibility { feature, .. } if feature == "reasoning_effort.none"
        ));
        let toggle = profile(
            Some(ReasoningProfile {
                efforts: vec![],
                can_disable: true,
            }),
            true,
        );
        let validated = validate(
            Some(&toggle),
            &options(Some(ReasoningEffort::None), None),
            &[],
        )
        .unwrap();
        assert_eq!(validated.reasoning, Some(ReasoningEffort::None));
        // 档位列表为空表示未知，不做限制。
        let any = validate(
            Some(&toggle),
            &options(Some(ReasoningEffort::Xhigh), None),
            &[],
        )
        .unwrap();
        assert_eq!(any.reasoning, Some(ReasoningEffort::Xhigh));
    }

    #[test]
    fn temperature_is_dropped_with_a_warning_when_the_model_rejects_it() {
        let profile = profile(None, false);
        let validated = validate(Some(&profile), &options(None, Some(0.3)), &[]).unwrap();
        assert_eq!(validated.temperature, None);
        assert!(matches!(
            &validated.warnings[0],
            ModelWarning::Unsupported { feature, .. } if feature == "temperature"
        ));
        let untouched = validate(Some(&profile), &options(None, None), &[]).unwrap();
        assert!(untouched.warnings.is_empty());
    }
}
