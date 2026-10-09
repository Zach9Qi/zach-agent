//! 纯内存诊断输出和脚本模型，不读取进程环境或访问网络。

use super::super::{args::Args, config::Config, output::Reporter, scenario::Scenario};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::{Arc, Mutex},
};
use zach_ai_core::{
    CallOptions, FinishReason, GenerateResult, LanguageModel, LanguageModelStream, ModelError,
    StreamAccumulator, StreamPart, Usage,
};

#[derive(Clone, Default)]
pub(super) struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    pub(super) fn lines(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

pub(super) fn output() -> (Arc<Reporter>, Buffer) {
    let buffer = Buffer::default();
    (
        Arc::new(Reporter::new(buffer.clone(), "test-secret".into())),
        buffer,
    )
}

pub(super) fn args(values: &[&str]) -> Args {
    let mut values: Vec<String> = values.iter().map(|s| (*s).into()).collect();
    if !values.iter().any(|s| s.starts_with("--scenario")) {
        values.extend(["--scenario".into(), "test.json".into()]);
    }
    Args::parse(&values).unwrap().unwrap()
}

pub(super) fn scenario(mode: &str) -> Scenario {
    let mut value = json!({
        "request": {"prompt": {"messages": [{"role":"user", "content":[{"type":"text", "text":"请求"}]}]}, "max_output_tokens":4096},
        "expect": [{"type":"finish_reason", "value":"stop"}, {"type":"text_nonempty"}]
    });
    if mode == "agent" {
        value["agent"] = json!({"tools":["add"], "next_tool_choice":{"type":"none"}});
        value["request"]["tool_choice"] = json!({"type":"tool","tool_name":"add"});
        value["expect"].as_array_mut().unwrap().extend([
            json!({"type":"tool_result", "name":"add", "value":{"sum":42}}),
            json!({"type":"replay", "content":"tool_results"}),
            json!({"type":"steps", "min":2,"max":4}),
        ]);
    }
    parse_scenario(mode, value)
}

pub(super) fn parse_scenario(mode: &str, value: Value) -> Scenario {
    Scenario::parse(
        config(&["--mode", mode]).mode,
        &value.to_string(),
        std::path::Path::new("scenarios/test.json"),
        |_| panic!("该场景不应读取文件"),
    )
    .unwrap()
}

pub(super) fn config(values: &[&str]) -> Config {
    let mut configs = Config::resolve(args(values), |name| match name {
        "PROBE_MODEL" => Some("test-model".into()),
        "PROBE_API_KEY" => Some("test-secret".into()),
        _ => None,
    })
    .unwrap();
    assert_eq!(configs.len(), 1, "测试辅助只用于单次运行配置");
    configs.remove(0)
}

pub(super) enum Script {
    Reply(Vec<StreamPart>),
    TransportFailure,
    Pending,
}

pub(super) struct Model {
    scripts: Mutex<VecDeque<Script>>,
    pub(super) requests: Mutex<Vec<CallOptions>>,
    pub(super) methods: Mutex<Vec<&'static str>>,
}

impl Model {
    pub(super) fn new(scripts: Vec<Script>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into()),
            requests: Mutex::new(vec![]),
            methods: Mutex::new(vec![]),
        })
    }

    fn next(&self, options: CallOptions) -> Script {
        self.requests.lock().unwrap().push(options);
        self.scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("出现未预期的模型请求")
    }
}

#[async_trait]
impl LanguageModel for Model {
    fn provider(&self) -> &str {
        "script"
    }
    fn model_id(&self) -> &str {
        "script-model"
    }

    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError> {
        self.methods.lock().unwrap().push("generate");
        match self.next(options) {
            Script::Reply(parts) => {
                let mut accumulator = StreamAccumulator::new();
                for part in parts {
                    accumulator.process(part);
                }
                Ok(accumulator.finish())
            }
            Script::Pending => futures::future::pending().await,
            Script::TransportFailure => Err(ModelError::StreamError {
                message: "传输失败".into(),
                source: None,
            }),
        }
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        self.methods.lock().unwrap().push("stream");
        Ok(match self.next(options) {
            Script::Reply(parts) => Box::pin(futures::stream::iter(parts.into_iter().map(Ok))),
            Script::Pending => Box::pin(futures::stream::pending()),
            Script::TransportFailure => Box::pin(futures::stream::iter(vec![
                Ok(StreamPart::TextDelta {
                    id: "t".into(),
                    delta: "半截".into(),
                    provider_metadata: None,
                }),
                Err(ModelError::StreamError {
                    message: "传输失败".into(),
                    source: None,
                }),
            ])),
        })
    }
}

pub(super) fn text_reply(text: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::TextDelta {
            id: "t".into(),
            delta: text.into(),
            provider_metadata: None,
        },
        finish(FinishReason::stop()),
    ]
}

pub(super) fn tool_reply(id: &str) -> Vec<StreamPart> {
    vec![
        StreamPart::ToolCall {
            tool_call_id: id.into(),
            tool_name: "add".into(),
            input: json!({"a":40,"b":2}).to_string(),
            provider_executed: false,
            dynamic: false,
            provider_metadata: None,
        },
        finish(FinishReason::tool_calls()),
    ]
}

fn finish(finish_reason: FinishReason) -> StreamPart {
    StreamPart::Finish {
        usage: Usage::simple(10, 5),
        finish_reason,
        provider_metadata: None,
    }
}
