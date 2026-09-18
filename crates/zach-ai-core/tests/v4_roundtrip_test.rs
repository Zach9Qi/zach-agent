//! 语言模型 V4 中间形态全链路与序列化测试

use bytes::Bytes;
use serde_json::json;
use zach_ai_core::{
    AssistantPart, CallOptions, FileData, FinishReason, GenerateResult, Message, ModelWarning,
    Prompt, ReasoningEffort, StreamAccumulator, StreamPart, ToolChoice, UnifiedFinishReason, Usage,
    UserPart,
};

#[test]
fn test_prompt_serialization_and_deserialization() {
    let mut prompt = Prompt::new();
    prompt.push(Message::system("你是一个专业的助手"));
    prompt.push(Message::user("帮我分析一下这张图片"));
    prompt.push(Message::User {
        content: vec![
            UserPart::text("请查看该图片附件："),
            UserPart::file(
                "image/png",
                FileData::from_bytes(Bytes::from_static(b"fake-image-binary-data")),
            ),
        ],
        provider_options: None,
    });

    let json_str = serde_json::to_string_pretty(&prompt).expect("序列化 Prompt 失败");
    let deserialized: Prompt = serde_json::from_str(&json_str).expect("反序列化 Prompt 失败");

    assert_eq!(prompt, deserialized);
    assert_eq!(prompt.len(), 3);
}

#[test]
fn test_file_data_base64_roundtrip() {
    let raw = Bytes::from_static(b"hello world \x00\x01\x02 binary");
    let file = FileData::from_bytes(raw.clone());

    let json_str = serde_json::to_string(&file).expect("序列化 FileData 失败");
    let deserialized: FileData = serde_json::from_str(&json_str).expect("反序列化 FileData 失败");

    if let FileData::Data { data } = deserialized {
        assert_eq!(data, raw);
    } else {
        panic!("反序列化类型不匹配");
    }
}

#[test]
fn test_call_options_builder() {
    let prompt = Prompt::new().with_user("你好，介绍一下 Rust 语言");
    let options = CallOptions::new(prompt)
        .with_temperature(0.7)
        .with_max_output_tokens(2048)
        .with_reasoning(ReasoningEffort::High)
        .with_tool_choice(ToolChoice::Auto);

    assert_eq!(options.temperature, Some(0.7));
    assert_eq!(options.max_output_tokens, Some(2048));
    assert_eq!(options.reasoning, Some(ReasoningEffort::High));
    assert_eq!(options.tool_choice, Some(ToolChoice::Auto));
}

#[test]
fn test_stream_accumulator_aggregates_complete_result() {
    let mut accumulator = StreamAccumulator::new();

    // 1. 模拟流开始
    accumulator.process(StreamPart::StreamStart {
        warnings: vec![ModelWarning::Compatibility {
            feature: "reasoning".to_string(),
            details: Some("降级为普通思考模式".to_string()),
        }],
    });

    // 2. 模拟思考链
    accumulator.process(StreamPart::ReasoningStart {
        id: "r-1".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ReasoningDelta {
        id: "r-1".to_string(),
        delta: "正在分析".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ReasoningDelta {
        id: "r-1".to_string(),
        delta: "用户需求...".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ReasoningEnd {
        id: "r-1".to_string(),
        provider_metadata: None,
    });

    // 3. 模拟正文文本流
    accumulator.process(StreamPart::TextStart {
        id: "t-1".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t-1".to_string(),
        delta: "你好！".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextDelta {
        id: "t-1".to_string(),
        delta: "我是 Rust AI Agent。".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::TextEnd {
        id: "t-1".to_string(),
        provider_metadata: None,
    });

    // 4. 模拟工具调用入参流
    accumulator.process(StreamPart::ToolInputStart {
        id: "call_abc".to_string(),
        tool_name: "calculator".to_string(),
        provider_executed: false,
        dynamic: false,
        title: None,
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_abc".to_string(),
        delta: "{\"expr\":".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputDelta {
        id: "call_abc".to_string(),
        delta: "\"1 + 1\"}".to_string(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ToolInputEnd {
        id: "call_abc".to_string(),
        provider_metadata: None,
    });

    // 5. 模拟完成
    accumulator.process(StreamPart::Finish {
        usage: Usage::simple(120, 45),
        finish_reason: FinishReason {
            unified: UnifiedFinishReason::ToolCalls,
            raw: Some("tool_calls".to_string()),
        },
        provider_metadata: None,
    });

    let result: GenerateResult = accumulator.finish();

    assert_eq!(result.text(), "你好！我是 Rust AI Agent。");
    assert_eq!(result.reasoning(), Some("正在分析用户需求...".to_string()));
    assert_eq!(result.finish_reason.unified, UnifiedFinishReason::ToolCalls);
    assert_eq!(result.usage.input_tokens.total, Some(120));
    assert_eq!(result.usage.output_tokens.total, Some(45));
    assert_eq!(result.warnings.len(), 1);

    // 6. 验证将输出自动转换为下一轮 Assistant 消息
    let next_assistant_msg = result.into_assistant_message();
    match next_assistant_msg {
        Message::Assistant { content, .. } => {
            assert_eq!(content.len(), 3); // Reasoning, Text, ToolCall
            match &content[2] {
                AssistantPart::ToolCall {
                    tool_call_id,
                    tool_name,
                    input,
                    ..
                } => {
                    assert_eq!(tool_call_id, "call_abc");
                    assert_eq!(tool_name, "calculator");
                    assert_eq!(input, &json!({"expr": "1 + 1"}));
                }
                _ => panic!("第 3 个 Part 期望为 ToolCall"),
            }
        }
        _ => panic!("期望为 Assistant 消息"),
    }
}
