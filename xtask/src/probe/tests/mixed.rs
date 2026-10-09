//! 混合附件进入三种模式，Agent 保留输入、推理元数据和关联的工具结果。

use super::{
    super::{
        expect,
        runner::run,
        trace::{Call, Outcome, Trace},
    },
    support::*,
};
use serde_json::json;
use zach_ai_core::{
    AssistantPart, CallOptions, Message, ModelCapabilities, ReasoningCapabilities, StreamPart,
    UnifiedFinishReason,
};

#[tokio::test]
async fn mixed_messages_and_reasoning_options_reach_every_mode_unchanged() {
    for mode in ["generate", "stream", "agent"] {
        let mut value = json!({"request":{
            "prompt":{"messages":[
                {"role":"system","content":"保留这个系统提示"},
                {"role":"user","content":[
                    {"type":"text","text":"读取这些附件"},
                    {"type":"file","media_type":"image/png","data":{"type":"data","data":"AQID"}},
                    {"type":"text","text":"再结合文档"},
                    {"type":"file","media_type":"application/pdf","filename":"a.pdf","data":{"type":"data","data":"BAUG"}},
                    {"type":"file","media_type":"image/png","data":{"type":"url","url":"https://example.com/b.png"}}
                ]},
                {"role":"user","content":[{"type":"text","text":"计算并回答"}]}
            ]}, "reasoning":"high", "provider_options":{"openai":{"reasoning":{"summary":"auto"}}}
        }, "expect":[{"type":"text_equals","value":"42"}]});
        if mode == "agent" {
            value["agent"] = json!({"tools":["add"],"next_tool_choice":{"type":"none"}});
            value["request"]["tool_choice"] = json!({"type":"tool","tool_name":"add"});
            value["expect"].as_array_mut().unwrap().extend([
                json!({"type":"replay","content":"input"}),
                json!({"type":"replay","content":"reasoning"}),
                json!({"type":"replay","content":"tool_results"}),
                json!({"type":"reasoning","evidence":"metadata"}),
                json!({"type":"event","event":"reasoning_delta","min":1,"requires":"reasoning_stream"}),
            ]);
        }
        let scenario = parse_scenario(mode, value);
        let expected = scenario.request.clone();
        let mut first = tool_reply("call_1");
        first.insert(
            0,
            StreamPart::ReasoningDelta {
                id: "r1".into(),
                delta: "计算".into(),
                provider_metadata: None,
            },
        );
        first.insert(1, StreamPart::ReasoningEnd { id:"r1".into(), provider_metadata:Some(
            serde_json::from_value(json!({"openai":{"responses_item":{"id":"r1","type":"reasoning","encrypted_content":"opaque"}}})).unwrap()
        ) });
        let scripts = if mode == "agent" {
            vec![Script::Reply(first), Script::Reply(text_reply("42"))]
        } else {
            vec![Script::Reply(text_reply("42"))]
        };
        let model = Model::new(scripts);
        let (reporter, _) = output();
        run(
            &config(&["--mode", mode]),
            &scenario,
            model.clone(),
            reporter,
        )
        .await
        .unwrap();
        let requests = model.requests.lock().unwrap();
        for request in requests.iter() {
            assert!(request
                .prompt
                .messages
                .starts_with(&expected.prompt.messages));
            assert_eq!(request.reasoning, expected.reasoning);
            assert_eq!(request.provider_options, expected.provider_options);
        }
        if mode != "agent" {
            assert_eq!(requests[0], expected);
        }
    }
}

#[test]
fn reasoning_replay_requires_exact_metadata_and_an_actual_next_request() {
    let metadata =
        serde_json::from_value(json!({"openai":{"encrypted_content":"opaque"}})).unwrap();
    let expected = AssistantPart::Reasoning {
        text: "推理".into(),
        provider_options: Some(metadata),
    };
    let mut accumulator = zach_ai_core::StreamAccumulator::new();
    accumulator.process(StreamPart::ReasoningStart {
        id: "r".into(),
        provider_metadata: None,
    });
    accumulator.process(StreamPart::ReasoningDelta {
        id: "r".into(),
        delta: "推理".into(),
        provider_metadata: None,
    });
    if let AssistantPart::Reasoning {
        provider_options, ..
    } = &expected
    {
        accumulator.process(StreamPart::ReasoningEnd {
            id: "r".into(),
            provider_metadata: provider_options.clone(),
        });
    }
    let result = accumulator.finish();
    let outcome = Outcome {
        text: "答案".into(),
        finish_reason: UnifiedFinishReason::Stop,
        messages: vec![],
        steps: 2,
    };
    let assertions = serde_json::from_value::<Vec<expect::Expectation>>(
        json!([{"type":"replay","content":"reasoning","requires":"reasoning_replay"}]),
    )
    .unwrap();
    for (part, passes) in [
        (expected.clone(), true),
        (AssistantPart::reasoning("推理"), false),
    ] {
        let trace = Trace {
            calls: vec![
                Call {
                    request: CallOptions::new(vec![Message::user("问题")]),
                    result: Some(result.clone()),
                    events: Default::default(),
                },
                Call {
                    request: CallOptions::new(vec![Message::assistant_tool_calls(vec![part])]),
                    result: None,
                    events: Default::default(),
                },
            ],
        };
        let (reporter, _) = output();
        assert_eq!(
            expect::verify(
                &assertions,
                ModelCapabilities {
                    reasoning: ReasoningCapabilities {
                        replay: true,
                        ..ReasoningCapabilities::default()
                    },
                },
                &outcome,
                &trace,
                &reporter,
            )
            .is_ok(),
            passes
        );
    }
    let empty = Trace::default();
    let (reporter, _) = output();
    assert!(expect::verify(
        &assertions,
        ModelCapabilities {
            reasoning: ReasoningCapabilities {
                replay: true,
                ..ReasoningCapabilities::default()
            },
        },
        &outcome,
        &empty,
        &reporter,
    )
    .is_err());
}

#[tokio::test]
async fn preloaded_tool_history_cannot_satisfy_new_execution_assertions() {
    let mut scenario = scenario("agent");
    scenario.request.prompt.messages.insert(
        0,
        Message::tool(vec![zach_ai_core::ToolPart::result_json(
            "old_call",
            "add",
            json!({"sum":42}),
        )]),
    );
    let (reporter, buffer) = output();
    assert!(run(
        &config(&["--mode", "agent"]),
        &scenario,
        Model::new(vec![Script::Reply(text_reply("42"))]),
        reporter
    )
    .await
    .is_err());
    let result = buffer
        .lines()
        .into_iter()
        .find(|v| v["type"] == "assertion_result" && v["data"]["expect"]["type"] == "tool_result")
        .unwrap();
    assert_eq!(result["data"]["success"], false);
}
