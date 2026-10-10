//! 通用推理档位到 Messages `thinking` 配置的映射。
//!
//! Claude 4.6 起支持自适应思考（`adaptive`）与 `output_config.effort`；更早的代际只认
//! `enabled` + `budget_tokens`。这里按模型 id 中的代际号选择形态，调用方仍可用
//! `provider_options.anthropic.thinking` 整体覆盖。
//!
//! 思考开启后 Anthropic 不接受 `temperature` 与 `top_k` 的修改（`top_p` 仍允许
//! 0.95 ~ 1），这两个参数会被丢弃并以 `Compatibility` 警告透出。

use serde_json::{json, Value};
use zach_ai_core::{ModelError, ModelWarning, ReasoningEffort};

use super::merge_value;

/// 自适应思考与 `effort` 参数起始的代际。
const ADAPTIVE_SINCE: (u32, u32) = (4, 6);

/// `budget_tokens` 的下限（API 约束）。
const MIN_BUDGET: u64 = 1024;

/// 把通用档位写入 `thinking` / `output_config.effort`。
///
/// `max_tokens` 用于约束旧代际的 `budget_tokens`（必须小于 `max_tokens`）。
pub(super) fn apply(
    body: &mut Value,
    model_id: &str,
    effort: ReasoningEffort,
    max_tokens: u64,
) -> Result<(), ModelError> {
    match effort {
        ReasoningEffort::ProviderDefault => return Ok(()),
        ReasoningEffort::None => {
            body["thinking"] = json!({ "type": "disabled" });
            return Ok(());
        }
        _ => {}
    }
    if supports_adaptive(model_id) {
        body["thinking"] = json!({ "type": "adaptive", "display": "summarized" });
        merge_value(
            &mut body["output_config"],
            json!({ "effort": effort_level(effort) }),
        );
        return Ok(());
    }
    // 旧代际：档位换算为思考预算，并压在 max_tokens 之下。
    if max_tokens <= MIN_BUDGET {
        return Err(ModelError::InvalidRequest(format!(
            "启用思考要求 max_tokens 大于 {MIN_BUDGET}，当前为 {max_tokens}"
        )));
    }
    let budget = budget_tokens(effort).min(max_tokens - 1);
    body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
    Ok(())
}

/// 思考处于开启状态时，去掉与之互斥的采样参数并给出警告。
///
/// 在厂商扩展合并之后调用，因此 `provider_options.anthropic.thinking` 的覆盖也会被考虑。
pub(super) fn reconcile_sampling(body: &mut Value, warnings: &mut Vec<ModelWarning>) {
    let active = matches!(
        body["thinking"]["type"].as_str(),
        Some("enabled" | "adaptive")
    );
    if !active {
        return;
    }
    for name in ["temperature", "top_k"] {
        if body.get(name).is_some() {
            body.as_object_mut().expect("请求正文是对象").remove(name);
            warnings.push(ModelWarning::Compatibility {
                feature: name.into(),
                details: Some(format!("开启思考时 Messages API 不接受 {name}，已忽略")),
            });
        }
    }
}

fn effort_level(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Minimal | ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::Xhigh => "xhigh",
        ReasoningEffort::Max => "max",
        ReasoningEffort::ProviderDefault | ReasoningEffort::None => {
            unreachable!("apply 已处理默认与关闭档位")
        }
    }
}

/// 旧代际的档位 → 思考预算（token）。数值取自常见实践，上限由 `max_tokens` 再压一层。
fn budget_tokens(effort: ReasoningEffort) -> u64 {
    match effort {
        ReasoningEffort::Minimal => MIN_BUDGET,
        ReasoningEffort::Low => 4_096,
        ReasoningEffort::Medium => 16_384,
        ReasoningEffort::High => 32_768,
        ReasoningEffort::Xhigh => 65_536,
        ReasoningEffort::Max => 131_072,
        ReasoningEffort::ProviderDefault | ReasoningEffort::None => {
            unreachable!("apply 已处理默认与关闭档位")
        }
    }
}

/// 代际未知（代理别名等）按最新形态处理。
fn supports_adaptive(model_id: &str) -> bool {
    generation(model_id).is_none_or(|generation| generation >= ADAPTIVE_SINCE)
}

/// 从模型 id 提取 `(主版本, 次版本)`：取前两个长度不超过 2 的纯数字段，
/// 跳过 `20250514` 这类日期后缀；只有一个数字段时次版本为 0。
pub(super) fn generation(model_id: &str) -> Option<(u32, u32)> {
    let mut numbers = model_id
        .split(['-', '.'])
        .filter(|segment| segment.len() <= 2 && segment.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|segment| segment.parse::<u32>().ok());
    let major = numbers.next()?;
    Some((major, numbers.next().unwrap_or(0)))
}

#[cfg(test)]
mod tests {
    //! 代际识别与档位到 thinking 配置的换算。
    use super::*;

    #[test]
    fn generation_is_read_from_the_model_id_and_ignores_date_suffixes() {
        for (id, expected) in [
            ("claude-opus-4-6", Some((4, 6))),
            ("claude-sonnet-4-5-20250929", Some((4, 5))),
            ("claude-sonnet-4-20250514", Some((4, 0))),
            ("claude-3-7-sonnet-20250219", Some((3, 7))),
            ("claude-3-5-haiku-latest", Some((3, 5))),
            ("claude-fable-5", Some((5, 0))),
            ("claude-fable-5-1", Some((5, 1))),
            ("claude-opus-4-1", Some((4, 1))),
            ("my-proxy-alias", None),
        ] {
            assert_eq!(generation(id), expected, "{id}");
        }
        assert!(supports_adaptive("claude-opus-4-6"));
        assert!(supports_adaptive("claude-fable-5-1"));
        assert!(supports_adaptive("my-proxy-alias"));
        assert!(!supports_adaptive("claude-sonnet-4-5"));
        assert!(!supports_adaptive("claude-3-7-sonnet-20250219"));
    }

    #[test]
    fn legacy_budget_is_capped_below_max_tokens_and_requires_headroom() {
        let mut body = json!({});
        apply(
            &mut body,
            "claude-sonnet-4-5",
            ReasoningEffort::High,
            64_000,
        )
        .unwrap();
        assert_eq!(
            body["thinking"],
            json!({"type": "enabled", "budget_tokens": 32_768})
        );
        assert!(body.get("output_config").is_none());
        let mut body = json!({});
        apply(
            &mut body,
            "claude-sonnet-4-5",
            ReasoningEffort::Xhigh,
            8_192,
        )
        .unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 8_191);
        let mut body = json!({});
        assert!(matches!(
            apply(&mut body, "claude-sonnet-4-5", ReasoningEffort::Low, 1_024),
            Err(ModelError::InvalidRequest(_))
        ));
        let mut body = json!({});
        apply(&mut body, "claude-sonnet-4-5", ReasoningEffort::None, 1_024).unwrap();
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
    }

    #[test]
    fn sampling_parameters_are_dropped_only_while_thinking_is_active() {
        let mut warnings = Vec::new();
        let mut body = json!({"thinking": {"type": "adaptive"}, "temperature": 0.5,
            "top_k": 40, "top_p": 0.97});
        reconcile_sampling(&mut body, &mut warnings);
        assert!(body.get("temperature").is_none());
        assert!(body.get("top_k").is_none());
        assert_eq!(body["top_p"], 0.97);
        assert_eq!(warnings.len(), 2);
        assert!(matches!(
            &warnings[0],
            ModelWarning::Compatibility { feature, .. } if feature == "temperature"
        ));
        let mut warnings = Vec::new();
        let mut body = json!({"thinking": {"type": "disabled"}, "temperature": 0.5});
        reconcile_sampling(&mut body, &mut warnings);
        assert_eq!(body["temperature"], 0.5);
        assert!(warnings.is_empty());
    }
}
