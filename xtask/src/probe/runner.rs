//! 协议无关的生成、流式和 Agent 联调流程。

use super::{
    agent,
    config::{Config, Mode},
    observe::ObservedModel,
    output::Reporter,
    ProbeResult,
};
use futures::StreamExt;
use serde_json::json;
use std::sync::Arc;
use tokio::time::{timeout, Instant};
use zach_ai_core::{LanguageModel, Message, StreamAccumulator, UnifiedFinishReason};

pub(super) async fn run(
    config: &Config,
    model: Arc<dyn LanguageModel>,
    reporter: Arc<Reporter>,
) -> ProbeResult<()> {
    reporter.emit(
        "probe_start",
        &json!({
            "protocol": config.protocol, "mode": config.mode,
            "model": config.model, "base_url": config.base_url,
            "timeout_secs": config.timeout.as_secs(),
        }),
    );
    reporter.check()?;
    let limit = if config.mode == Mode::Agent {
        config.max_steps
    } else {
        1
    };
    let model = Arc::new(ObservedModel::new(model, reporter.clone(), limit));
    let start = Instant::now();
    let result = timeout(config.timeout, async {
        if config.mode == Mode::Agent {
            agent::run(config, model, reporter.clone()).await
        } else {
            single(config, model, &reporter).await
        }
    })
    .await
    .unwrap_or_else(|_| {
        Err(format!(
            "联调超过 {} 秒，已停止本地调用",
            config.timeout.as_secs()
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
    config: &Config,
    model: Arc<dyn LanguageModel>,
    reporter: &Reporter,
) -> ProbeResult<()> {
    let mut options = config.options.clone();
    options.prompt = vec![Message::user(&config.prompt)].into();
    let result = match config.mode {
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
            let result = accumulator.finish();
            reporter.emit("model_result", &result);
            result
        }
        Mode::Agent => unreachable!("Agent 由独立流程处理"),
    };
    require_stop(result.finish_reason.unified)?;
    if result.text().trim().is_empty() {
        return Err("模型正常收尾但没有文本输出，请检查事件和模型配置".into());
    }
    Ok(())
}

pub(super) fn require_stop(reason: UnifiedFinishReason) -> ProbeResult<()> {
    if reason == UnifiedFinishReason::Stop {
        Ok(())
    } else {
        Err(format!(
            "联调未正常完成，结束原因: {reason:?}；请检查前面的事件和用量"
        ))
    }
}
