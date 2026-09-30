//! `Usage` 累加语义

use serde_json::json;
use zach_ai_core::{InputTokenUsage, OutputTokenUsage, Usage};

#[test]
fn add_sums_present_fields_and_keeps_missing_as_none() {
    let mut total = Usage {
        input_tokens: InputTokenUsage {
            total: Some(10),
            cache_read: Some(4),
            ..Default::default()
        },
        output_tokens: OutputTokenUsage {
            total: Some(5),
            ..Default::default()
        },
        raw: Some(json!({ "first": true })),
    };
    let other = Usage {
        input_tokens: InputTokenUsage {
            total: Some(3),
            no_cache: Some(2),
            ..Default::default()
        },
        output_tokens: OutputTokenUsage {
            total: Some(7),
            reasoning: Some(1),
            ..Default::default()
        },
        raw: Some(json!({ "second": true })),
    };

    total.add(&other);

    assert_eq!(total.input_tokens.total, Some(13));
    assert_eq!(total.input_tokens.no_cache, Some(2));
    assert_eq!(total.input_tokens.cache_read, Some(4));
    assert_eq!(total.input_tokens.cache_write, None);
    assert_eq!(total.output_tokens.total, Some(12));
    assert_eq!(total.output_tokens.text, None);
    assert_eq!(total.output_tokens.reasoning, Some(1));
    // 厂商原始数据不参与累加
    assert_eq!(total.raw, Some(json!({ "first": true })));
}

#[test]
fn add_assign_matches_add() {
    let mut a = Usage::simple(1, 2);
    let mut b = Usage::simple(1, 2);
    let delta = Usage::simple(10, 20);
    a.add(&delta);
    b += &delta;
    assert_eq!(a, b);
    assert_eq!(a, Usage::simple(11, 22));
}
