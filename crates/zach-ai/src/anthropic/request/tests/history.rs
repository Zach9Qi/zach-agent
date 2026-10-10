//! Messages 历史回放：角色合并、工具结果、附件与思考块。

use super::*;
use serde_json::json;
use zach_ai_core::{
    AssistantPart, FileData, Message, ToolPart, ToolResultContentBlock, ToolResultOutput, UserPart,
};

#[test]
fn tool_results_follow_tool_calls_and_merge_into_alternating_roles() {
    let options = CallOptions::new(vec![
        Message::user("1+2"),
        Message::assistant_tool_calls(vec![
            AssistantPart::text("我来算 "),
            AssistantPart::tool_call("toolu_1", "add", json!({"a": 1, "b": 2})),
        ]),
        Message::tool(vec![ToolPart::result_json(
            "toolu_1",
            "add",
            json!({"sum": 3}),
        )]),
        Message::user("谢谢"),
        Message::assistant("不客气  "),
    ]);
    let messages = build(&options)["messages"].clone();
    assert_eq!(messages.as_array().unwrap().len(), 4);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        messages[1]["content"],
        json!([
            {"type": "text", "text": "我来算 "},
            {"type": "tool_use", "id": "toolu_1", "name": "add", "input": {"a": 1, "b": 2}}
        ])
    );
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(
        messages[2]["content"],
        json!([
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "{\"sum\":3}"},
            {"type": "text", "text": "谢谢"}
        ])
    );
    // 末尾助手预填充不能以空白结尾。
    assert_eq!(messages[3]["content"][0]["text"], "不客气");
}

#[test]
fn tool_result_errors_denials_and_composite_content_are_mapped() {
    let parts = vec![
        ToolPart::ToolResult {
            tool_call_id: "a".into(),
            tool_name: "t".into(),
            output: ToolResultOutput::error_text("失败"),
            provider_options: None,
        },
        ToolPart::ToolResult {
            tool_call_id: "b".into(),
            tool_name: "t".into(),
            output: ToolResultOutput::denied(None::<String>),
            provider_options: None,
        },
        ToolPart::ToolResult {
            tool_call_id: "c".into(),
            tool_name: "t".into(),
            output: ToolResultOutput::Content {
                value: vec![
                    ToolResultContentBlock::Text {
                        text: "图".into(),
                        provider_options: None,
                    },
                    ToolResultContentBlock::File {
                        media_type: "image/png".into(),
                        data: FileData::from_bytes(vec![1, 2, 3]),
                        filename: None,
                        provider_options: None,
                    },
                ],
            },
            provider_options: None,
        },
    ];
    let options = CallOptions::new(vec![
        Message::user("x"),
        Message::assistant_tool_calls(vec![AssistantPart::tool_call("a", "t", json!({}))]),
        Message::tool(parts),
    ]);
    let content = build(&options)["messages"][2]["content"].clone();
    assert_eq!(content[0]["is_error"], true);
    assert_eq!(content[0]["content"], "失败");
    assert_eq!(content[1]["is_error"], true);
    assert_eq!(content[1]["content"], "工具执行被拒绝");
    assert!(content[2].get("is_error").is_none());
    assert_eq!(
        content[2]["content"][0],
        json!({"type": "text", "text": "图"})
    );
    assert_eq!(content[2]["content"][1]["type"], "image");
    assert_eq!(content[2]["content"][1]["source"]["data"], "AQID");
}

#[test]
fn attachments_map_to_image_and_document_blocks() {
    let options = CallOptions::new(vec![Message::User {
        content: vec![
            UserPart::file("image/png", FileData::from_url("https://example.com/a.png")),
            UserPart::file(
                "image/jpeg",
                FileData::from_reference("anthropic", "file_1"),
            ),
            UserPart::File {
                media_type: "application/pdf".into(),
                data: FileData::from_bytes(vec![1, 2, 3]),
                filename: Some("报告.pdf".into()),
                provider_options: None,
            },
            UserPart::file("text/plain", FileData::from_text("内联")),
            UserPart::file(
                "text/markdown",
                FileData::from_bytes("# 标题".as_bytes().to_vec()),
            ),
        ],
        provider_options: None,
    }]);
    let content = build(&options)["messages"][0]["content"].clone();
    assert_eq!(
        content[0],
        json!({"type": "image", "source": {"type": "url", "url": "https://example.com/a.png"}})
    );
    assert_eq!(
        content[1]["source"],
        json!({"type": "file", "file_id": "file_1"})
    );
    assert_eq!(
        content[2],
        json!({"type": "document", "title": "报告.pdf",
            "source": {"type": "base64", "media_type": "application/pdf", "data": "AQID"}})
    );
    assert_eq!(
        content[3]["source"],
        json!({"type": "text", "media_type": "text/plain", "data": "内联"})
    );
    assert_eq!(content[4]["source"]["data"], "# 标题");
    for part in [
        UserPart::file("audio/wav", FileData::from_bytes(vec![1])),
        UserPart::file("image/png", FileData::from_reference("openai", "file-1")),
        UserPart::file("image/png", FileData::from_text("不是图片")),
    ] {
        let options = CallOptions::new(vec![Message::User {
            content: vec![part],
            provider_options: None,
        }]);
        let error = build_request("example", None, &options, false).unwrap_err();
        assert!(
            matches!(
                error,
                ModelError::UnsupportedFeature { .. } | ModelError::InvalidRequest(_)
            ),
            "{error:?}"
        );
    }
}

#[test]
fn reasoning_replays_only_with_signature_or_redacted_data() {
    let options = CallOptions::new(vec![
        Message::user("x"),
        Message::Assistant {
            content: vec![
                AssistantPart::reasoning("无签名"),
                AssistantPart::Reasoning {
                    text: "有签名".into(),
                    provider_options: anthropic_options(json!({"signature": "sig"})),
                },
                AssistantPart::Reasoning {
                    text: String::new(),
                    provider_options: anthropic_options(json!({"redacted_thinking": "opaque"})),
                },
                AssistantPart::text("答案"),
            ],
            provider_options: None,
        },
    ]);
    assert_eq!(
        build(&options)["messages"][1]["content"],
        json!([
            {"type": "thinking", "thinking": "有签名", "signature": "sig"},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "答案"}
        ])
    );
    // 只剩无签名推理的助手消息整体省略，相邻用户消息随之合并。
    let only = CallOptions::new(vec![
        Message::user("a"),
        Message::Assistant {
            content: vec![AssistantPart::reasoning("无签名")],
            provider_options: None,
        },
        Message::user("b"),
    ]);
    let messages = build(&only)["messages"].clone();
    assert_eq!(messages.as_array().unwrap().len(), 1);
    assert_eq!(messages[0]["content"].as_array().unwrap().len(), 2);
}
