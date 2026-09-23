//! 类型化工具：入参类型自动生成 JSON Schema 并完成反序列化校验

use crate::tool::context::ToolContext;
use crate::tool::error::ToolError;
use crate::tool::types::{AgentTool, SharedTool, ToolExecutionMode, ToolOutcome};
use async_trait::async_trait;
use schemars::gen::SchemaSettings;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
use zach_ai_core::FunctionTool;

/// 以强类型入参定义的工具，经 [`Typed`] 适配为 [`AgentTool`]
#[async_trait]
pub trait TypedTool: Send + Sync + 'static {
    /// 入参类型，其 JSON Schema 即工具的参数声明
    type Input: DeserializeOwned + JsonSchema + Send + 'static;

    /// 工具名称
    fn name(&self) -> &str;

    /// 给模型看的用途说明
    fn description(&self) -> &str;

    /// 界面展示用的友好标题
    fn title(&self) -> Option<&str> {
        None
    }

    /// 单工具执行方式
    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        None
    }

    /// 本次调用是否需要人工审批
    fn needs_approval(&self, input: &Self::Input) -> bool {
        let _ = input;
        false
    }

    /// 执行工具
    async fn call(&self, input: Self::Input, ctx: ToolContext) -> Result<ToolOutcome, ToolError>;
}

/// [`TypedTool`] 到 [`AgentTool`] 的适配器
pub struct Typed<T> {
    tool: T,
    definition: FunctionTool,
}

impl<T: TypedTool> Typed<T> {
    /// 生成参数声明并包装工具
    pub fn new(tool: T) -> Self {
        let definition = FunctionTool::new(tool.name(), input_schema::<T::Input>())
            .with_description(tool.description());
        Self { tool, definition }
    }

    /// 访问被包装的工具
    pub fn inner(&self) -> &T {
        &self.tool
    }
}

/// 便捷包装为可共享的工具句柄
pub fn typed_tool<T: TypedTool>(tool: T) -> SharedTool {
    Arc::new(Typed::new(tool))
}

#[async_trait]
impl<T: TypedTool> AgentTool for Typed<T> {
    fn definition(&self) -> &FunctionTool {
        &self.definition
    }

    fn title(&self) -> Option<&str> {
        self.tool.title()
    }

    fn execution_mode(&self) -> Option<ToolExecutionMode> {
        self.tool.execution_mode()
    }

    fn prepare_input(&self, input: Value) -> Result<Value, ToolError> {
        parse::<T::Input>(input.clone()).map(|_| input)
    }

    fn needs_approval(&self, input: &Value) -> bool {
        parse::<T::Input>(input.clone())
            .map(|typed| self.tool.needs_approval(&typed))
            .unwrap_or(false)
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let typed = parse::<T::Input>(input)?;
        self.tool.call(typed, ctx).await
    }
}

fn parse<I: DeserializeOwned>(input: Value) -> Result<I, ToolError> {
    serde_json::from_value(input).map_err(ToolError::invalid_input)
}

/// 子类型全部内联，避免部分厂商不支持 `$ref`
fn input_schema<I: JsonSchema>() -> Value {
    let mut settings = SchemaSettings::draft07();
    settings.inline_subschemas = true;
    let root = settings.into_generator().into_root_schema_for::<I>();
    let mut schema =
        serde_json::to_value(root).unwrap_or_else(|_| Value::Object(Default::default()));
    if let Value::Object(map) = &mut schema {
        map.remove("$schema");
    }
    schema
}
