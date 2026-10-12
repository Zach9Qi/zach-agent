//! v1 样例文件：冻结落盘格式。任何让它们无法加载、或重新序列化后发生变化的改动，
//! 都意味着破坏了已有会话，必须提升格式版本并编写迁移（见 `docs/session-persistence.md`）

use super::tool_outputs;
use serde_json::{json, Value};
use zach_ai_core::Message;
use zach_coding_agent::session::{Entry, EntryBody, ModelRef, SessionHeader, SessionLog};

const FIXTURES: [&str; 4] = ["linear", "compacted", "branched", "interrupted"];

/// 样例文件的各行
fn lines(name: &str) -> Vec<String> {
    let path = format!(
        "{}/tests/fixtures/sessions/v1/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    text.lines().map(str::to_string).collect()
}

/// 解析样例文件
fn load(name: &str) -> (SessionHeader, SessionLog) {
    let lines = lines(name);
    let header: SessionHeader = serde_json::from_str(&lines[0]).unwrap();
    let entries: Vec<Entry> = lines[1..]
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (header, SessionLog::from_entries(entries).unwrap())
}

/// 每一行解析后再序列化，结构与原文完全一致
#[test]
fn every_v1_fixture_round_trips_unchanged() {
    for name in FIXTURES {
        let lines = lines(name);
        let header: SessionHeader = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(header.format_version, 1);
        let original: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(
            serde_json::to_value(&header).unwrap(),
            original,
            "{name} 文件头"
        );
        for (number, line) in lines.iter().enumerate().skip(1) {
            let entry: Entry = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{name} 第 {} 行: {error}", number + 1));
            let original: Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                serde_json::to_value(&entry).unwrap(),
                original,
                "{name} 第 {} 行",
                number + 1
            );
        }
    }
}

#[test]
fn linear_fixture_restores_context_config_and_tool_details() {
    let (header, log) = load("linear");
    assert_eq!(header.workspace_root.as_deref(), Some("/repo"));
    assert_eq!(log.leaf(), Some(8));

    let context = log.context();
    assert_eq!(context.len(), 6);
    assert_eq!(context[0], Message::user("把 a.txt 里的 1 改成 2"));
    assert_eq!(context[5], Message::assistant("改好了"));

    let config = log.config();
    assert_eq!(
        config.model,
        Some(ModelRef {
            provider: "anthropic".into(),
            id: "claude-test".into()
        })
    );
    assert_eq!(
        config.tools,
        Some(vec!["read".to_string(), "edit".to_string()])
    );

    let EntryBody::Message(edit) = &log.get(6).unwrap().body else {
        panic!("第 6 条应为消息");
    };
    assert_eq!(edit.tool_details["c2"]["first_changed_line"], json!(1));
}

#[test]
fn compacted_fixture_starts_with_the_summary() {
    let (_, log) = load("compacted");
    let context = log.context();
    let Message::User { content, .. } = &context[0] else {
        panic!("摘要应为用户消息");
    };
    assert!(format!("{content:?}").contains("用户先后问了两个问题"));
    assert_eq!(
        &context[1..],
        [
            Message::user("第二问"),
            Message::assistant("第二答"),
            Message::user("第三问"),
            Message::assistant("第三答"),
        ]
    );
}

#[test]
fn branched_fixture_follows_the_checked_out_branch() {
    let (_, log) = load("branched");
    assert_eq!(log.leaf(), Some(7));
    assert_eq!(
        log.context(),
        [
            Message::user("原始问题"),
            Message::assistant("第一版回答"),
            Message::user("追问 B"),
            Message::assistant("回答 B"),
        ]
    );
}

#[test]
fn interrupted_fixture_repairs_dangling_calls() {
    let (_, log) = load("interrupted");
    let context = log.context();
    assert_eq!(context.len(), 4);
    let interrupted = "运行中断，工具未得出结果".to_string();
    assert_eq!(
        tool_outputs(&context[2]),
        [
            ("c1".to_string(), interrupted.clone()),
            ("c2".to_string(), interrupted),
        ]
    );
    assert_eq!(context[3], Message::user("继续"));
}
