//! 包装统一模型接口，记录实际调用参数并限制联调调用次数。

use super::output::Reporter;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use zach_ai_core::{
    CallOptions, GenerateResult, LanguageModel, LanguageModelStream, ModelError, ModelProfile,
};

pub(super) struct ObservedModel {
    inner: Arc<dyn LanguageModel>,
    reporter: Arc<Reporter>,
    calls: AtomicUsize,
    limit: usize,
}

impl ObservedModel {
    pub(super) fn new(
        inner: Arc<dyn LanguageModel>,
        reporter: Arc<Reporter>,
        limit: usize,
    ) -> Self {
        Self {
            inner,
            reporter,
            calls: AtomicUsize::new(0),
            limit,
        }
    }

    fn request(&self, options: &CallOptions, mode: &str) -> Result<(), ModelError> {
        let call = self.calls.fetch_add(1, Ordering::Relaxed);
        if call >= self.limit {
            return Err(ModelError::InvalidRequest(format!(
                "达到联调模型调用上限 {}",
                self.limit
            )));
        }
        self.reporter.emit(
            "model_request",
            &json!({
                "call": call + 1, "mode": mode, "model": self.model_id(),
                "provider": self.provider(), "options": options,
            }),
        );
        self.reporter.check().map_err(ModelError::Other)
    }

    fn error(&self, error: &ModelError) {
        report_error(&self.reporter, error);
    }
}

#[async_trait]
impl LanguageModel for ObservedModel {
    fn provider(&self) -> &str {
        self.inner.provider()
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn profile(&self) -> Option<&ModelProfile> {
        self.inner.profile()
    }
    fn is_url_supported(&self, media_type: &str, url: &str) -> bool {
        self.inner.is_url_supported(media_type, url)
    }

    async fn do_generate(&self, options: CallOptions) -> Result<GenerateResult, ModelError> {
        self.request(&options, "generate")?;
        match self.inner.do_generate(options).await {
            Ok(result) => {
                self.reporter.emit("model_result", &result);
                Ok(result)
            }
            Err(error) => {
                self.error(&error);
                Err(error)
            }
        }
    }

    async fn do_stream(&self, options: CallOptions) -> Result<LanguageModelStream, ModelError> {
        self.request(&options, "stream")?;
        let stream = match self.inner.do_stream(options).await {
            Ok(stream) => stream,
            Err(error) => {
                self.error(&error);
                return Err(error);
            }
        };
        let reporter = self.reporter.clone();
        Ok(Box::pin(stream.inspect(move |part| match part {
            Ok(part) => reporter.emit("stream_part", part),
            Err(error) => report_error(&reporter, error),
        })))
    }
}

fn report_error(reporter: &Reporter, error: &ModelError) {
    let raw = match error {
        ModelError::ProviderError { raw, .. } => raw.as_ref(),
        _ => None,
    };
    reporter.emit(
        "model_error",
        &json!({
            "message": error.to_string(),
            "source": std::error::Error::source(error).map(ToString::to_string),
            "raw": raw,
        }),
    );
}
