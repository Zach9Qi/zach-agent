//! 协议无关的生成、流式和 Agent 联调流程。

use super::{
    agent,
    config::{Config, Mode},
    expect,
    observe::ObservedModel,
    output::Reporter,
    scenario::Scenario,
    trace::{Outcome, SharedTrace},
    ProbeResult,
};
use futures::StreamExt;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::time::{timeout, Instant};
use zach_ai_core::{LanguageModel, StreamAccumulator, UnifiedFinishReason};

pub(super) async fn run(
    config: &Config,
    scenario: &Scenario,
    model: Arc<dyn LanguageModel>,
    reporter: Arc<Reporter>,
) -> ProbeResult<()> {
    reporter.emit(
        "probe_start",
        &json!({
            "protocol": config.protocol, "mode": config.mode,
            "model": config.model, "base_url": config.base_url,
            "scenario": config.scenario_path, "timeout_secs": scenario.timeout_secs,
        }),
    );
    reporter.check()?;
    let trace = SharedTrace::default();
    let model = Arc::new(ObservedModel::new(
        model,
        reporter.clone(),
        scenario.max_steps(config.mode),
        trace.clone(),
    ));
    let capabilities = model.capabilities();
    let start = Instant::now();
    let result = timeout(Duration::from_secs(scenario.timeout_secs), async {
        let outcome = if config.mode == Mode::Agent {
            agent::run(scenario, model, reporter.clone()).await?
        } else {
            single(config.mode, scenario, model, &reporter).await?
        };
        let trace = trace.lock().expect("观察记录锁中毒");
        // 任何一轮错误或缺少结束事件都不能被宽松的内容断言掩盖。
        for call in &trace.calls {
            let result = call.result.as_ref().ok_or("模型响应没有完整收尾")?;
            require_complete(result.finish_reason.unified)?;
        }
        expect::verify(&scenario.expect, capabilities, &outcome, &trace, &reporter)
    })
    .await
    .unwrap_or_else(|_| {
        Err(format!(
            "联调超过 {} 秒，已停止本地调用",
            scenario.timeout_secs
        ))
    });
    reporter.emit(
        "probe_finish",
        &json!({
            "success": result.is_ok(), "elapsed_ms": start.elapsed().as_millis(),
        }),
    );
    reporter.check()?;
    result
}

async fn single(
    mode: Mode,
    scenario: &Scenario,
    model: Arc<dyn LanguageModel>,
    reporter: &Reporter,
) -> ProbeResult<Outcome> {
    let options = scenario.request.clone();
    let result = match mode {
        Mode::Generate => model
            .do_generate(options)
            .await
            .map_err(|e| e.to_string())?,
        Mode::Stream => {
            let mut stream = model.do_stream(options).await.map_err(|e| e.to_string())?;
            let mut accumulator = StreamAccumulator::new();
            while let Some(part) = stream.next().await {
                match part {
                    Ok(part) => accumulator.process(part),
                    Err(error) => {
                        reporter.emit("partial_result", &accumulator.finish());
                        return Err(error.to_string());
                    }
                }
                reporter.check()?;
            }
            accumulator.finish()
        }
        Mode::Agent => unreachable!("Agent 由独立流程处理"),
    };
    Ok(Outcome {
        text: result.text(),
        finish_reason: result.finish_reason.unified,
        messages: vec![result.into_assistant_message()],
        steps: 1,
    })
}

fn require_complete(reason: UnifiedFinishReason) -> ProbeResult<()> {
    if matches!(
        reason,
        UnifiedFinishReason::Error | UnifiedFinishReason::Unknown
    ) {
        Err(format!(
            "联调未正常完成，结束原因: {reason:?}；请检查前面的事件和用量"
        ))
    } else {
        Ok(())
    }
}
