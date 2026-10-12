//! 条目日志：树结构与当前叶子、回退、不变量校验、配置合并，以及投影中的压缩与崩溃修复

use super::{calls, input, reply, results, tool_outputs};
use serde_json::json;
use zach_ai_core::Message;
use zach_coding_agent::session::{
    CompactionEntry, CompactionReason, ConfigEntry, CustomEntry, Entry, EntryBody, LogError,
    ModelRef, SessionLog,
};

fn log_of(bodies: Vec<EntryBody>) -> SessionLog {
    let mut log = SessionLog::new();
    for body in bodies {
        log.append(body, 0).unwrap();
    }
    log
}

fn compaction(summary: &str, first_kept: Option<u64>) -> EntryBody {
    EntryBody::Compaction(CompactionEntry {
        summary: summary.into(),
        first_kept,
        tokens_before: 1000,
        reason: CompactionReason::Manual,
        read_files: Vec::new(),
        modified_files: Vec::new(),
    })
}

/// 上下文中各消息的纯文本（工具消息记为 `tool`）
fn texts(log: &SessionLog) -> Vec<String> {
    log.context()
        .iter()
        .map(|message| match message {
            Message::User { content, .. } => format!("{content:?}"),
            Message::Assistant { .. } => "assistant".to_string(),
            Message::Tool { .. } => "tool".to_string(),
            Message::System { content, .. } => content.clone(),
        })
        .collect()
}

#[test]
fn tree_entries_chain_onto_the_current_leaf() {
    let log = log_of(vec![input("问"), reply("答")]);
    let entries = log.entries();
    assert_eq!((entries[0].id, entries[0].parent), (1, None));
    assert_eq!((entries[1].id, entries[1].parent), (2, Some(1)));
    assert_eq!(log.leaf(), Some(2));
    assert_eq!(
        log.context(),
        [Message::user("问"), Message::assistant("答")]
    );
}

#[test]
fn checkout_moves_the_leaf_and_later_entries_branch_from_it() {
    let mut log = log_of(vec![input("原问题"), reply("答"), input("追问 A")]);
    let checkout = log.append(EntryBody::Checkout { target: 2 }, 0).unwrap();
    assert_eq!(checkout.parent, None);
    assert_eq!(log.leaf(), Some(2));

    let branched = log.append(input("追问 B"), 0).unwrap();
    assert_eq!((branched.id, branched.parent), (5, Some(2)));
    assert_eq!(log.context().last(), Some(&Message::user("追问 B")));

    let old: Vec<u64> = log.branch_to(3).iter().map(|entry| entry.id).collect();
    assert_eq!(old, [1, 2, 3]);
    assert_eq!(log.entries().len(), 5);
}

/// 失败的追加不改变日志
#[test]
fn invalid_checkout_targets_are_rejected() {
    let mut log = log_of(vec![input("问")]);
    let missing = log.append(EntryBody::Checkout { target: 9 }, 0);
    assert_eq!(
        missing.unwrap_err(),
        LogError::InvalidCheckout { target: 9 }
    );

    log.append(EntryBody::Checkout { target: 1 }, 0).unwrap();
    let control = log.append(EntryBody::Checkout { target: 2 }, 0);
    assert_eq!(
        control.unwrap_err(),
        LogError::InvalidCheckout { target: 2 }
    );
    assert_eq!(log.entries().len(), 2);
}

#[test]
fn rebuilding_rejects_broken_invariants() {
    let entry = |id, parent| Entry {
        id,
        parent,
        ts: 0,
        body: input("x"),
    };
    let repeated = SessionLog::from_entries(vec![entry(1, None), entry(1, None)]);
    assert_eq!(repeated.unwrap_err(), LogError::NonIncreasingId { id: 1 });

    let orphan = SessionLog::from_entries(vec![entry(1, None), entry(2, Some(7))]);
    assert_eq!(
        orphan.unwrap_err(),
        LogError::MissingParent { id: 2, parent: 7 }
    );

    let rebuilt = SessionLog::from_entries(vec![entry(1, None), entry(3, Some(1))]).unwrap();
    assert_eq!(rebuilt.leaf(), Some(3));
}

#[test]
fn config_changes_merge_along_the_current_branch_only() {
    let model = ModelRef {
        provider: "p".into(),
        id: "m".into(),
    };
    let mut log = log_of(vec![
        EntryBody::Config(ConfigEntry {
            model: Some(model.clone()),
            system_prompt: Some("旧提示词".into()),
            ..ConfigEntry::default()
        }),
        input("问"),
        EntryBody::Config(ConfigEntry {
            system_prompt: Some("新提示词".into()),
            ..ConfigEntry::default()
        }),
    ]);
    let config = log.config();
    assert_eq!(config.model, Some(model));
    assert_eq!(config.system_prompt.as_deref(), Some("新提示词"));

    log.append(EntryBody::Checkout { target: 2 }, 0).unwrap();
    assert_eq!(log.config().system_prompt.as_deref(), Some("旧提示词"));
}

#[test]
fn config_and_custom_entries_stay_out_of_the_context() {
    let log = log_of(vec![
        EntryBody::Config(ConfigEntry::default()),
        input("问"),
        EntryBody::Custom(CustomEntry {
            namespace: "host".into(),
            data: json!(1),
        }),
    ]);
    assert_eq!(log.context(), [Message::user("问")]);
}

#[test]
fn compaction_replaces_messages_before_the_kept_region() {
    let log = log_of(vec![
        input("一"),
        reply("一答"),
        input("二"),
        reply("二答"),
        compaction("摘要甲", Some(3)),
        input("三"),
    ]);
    let context = texts(&log);
    assert!(context[0].contains("摘要甲") && context[0].contains("<summary>"));
    assert_eq!(context.len(), 4);
    assert_eq!(log.context()[1], Message::user("二"));
    assert_eq!(log.context()[3], Message::user("三"));
}

#[test]
fn only_the_latest_compaction_applies_and_may_keep_nothing() {
    let log = log_of(vec![
        input("一"),
        compaction("摘要甲", Some(1)),
        input("二"),
        compaction("摘要乙", None),
        input("三"),
    ]);
    let context = texts(&log);
    assert_eq!(context.len(), 2);
    assert!(context[0].contains("摘要乙") && !context[0].contains("摘要甲"));
    assert_eq!(log.context()[1], Message::user("三"));
}

/// 进程在工具执行中途被杀：悬空的调用补上"运行中断"结果，已有结果保持不变
#[test]
fn dangling_tool_calls_get_interrupted_results() {
    let partial = log_of(vec![input("问"), calls(&["c1", "c2"]), results(&["c1"])]);
    assert_eq!(
        tool_outputs(&partial.context()[2]),
        [
            ("c1".to_string(), "ok".to_string()),
            ("c2".to_string(), "运行中断，工具未得出结果".to_string()),
        ]
    );

    let missing = log_of(vec![input("问"), calls(&["c1"]), input("继续")]);
    let context = missing.context();
    assert_eq!(context.len(), 4);
    assert_eq!(
        tool_outputs(&context[2]),
        [("c1".to_string(), "运行中断，工具未得出结果".to_string())]
    );
    assert_eq!(context[3], Message::user("继续"));
}
