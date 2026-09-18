//! 工具注册中心与动态加载管理

use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zach_ai_core::{FunctionTool, ToolDefinition, ToolResultOutput};
use crate::error::AgentError;
use crate::tool::executor::ToolExecutor;

/// 延迟工具加载器：在动态加载时提供完整的定义与执行器
#[async_trait]
pub trait DeferredLoader: Send + Sync {
    /// 加载工具的完整定义与执行器
    async fn load(
        &self,
        name: &str,
    ) -> Result<(FunctionTool, Option<Arc<dyn ToolExecutor>>), AgentError>;
}

/// 基于异步闭包的延迟加载器
pub struct AsyncDeferredLoader<F> {
    loader: F,
}

impl<F> AsyncDeferredLoader<F> {
    /// 构造新的异步延迟加载器
    pub fn new(loader: F) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl<F, Fut> DeferredLoader for AsyncDeferredLoader<F>
where
    F: Fn(&str) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<(FunctionTool, Option<Arc<dyn ToolExecutor>>), AgentError>>
        + Send
        + 'static,
{
    async fn load(
        &self,
        name: &str,
    ) -> Result<(FunctionTool, Option<Arc<dyn ToolExecutor>>), AgentError> {
        (self.loader)(name).await
    }
}

/// Agent 运行时的延迟工具轻量元信息
///
/// 仅记录工具名称、用途说明、所属命名空间等，不包含具体的 input_schema。
/// 用于工具目录展示与模型动态发现。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeferredToolMeta {
    /// 工具唯一标识名称
    pub name: String,
    /// 给模型看的工具用途简要说明
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 所属命名空间或动态数据源（如 MCP 服务名、插件分类等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// 附加元数据（如标签、端点地址、版本等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
}

impl DeferredToolMeta {
    /// 构造新的延迟工具元信息
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            namespace: None,
            metadata: None,
        }
    }

    /// 设置工具描述
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 设置命名空间
    pub fn with_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    /// 设置元数据
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = Some(metadata);
        self
    }
}

/// 已激活的活跃工具条目
struct ActiveEntry {
    definition: FunctionTool,
    executor: Option<Arc<dyn ToolExecutor>>,
}

/// 延迟加载工具条目
struct DeferredEntry {
    meta: DeferredToolMeta,
    loader: Arc<dyn DeferredLoader>,
}

/// 工具注册中心（ToolRegistry）
///
/// 统一管理已加载的标准函数工具与未加载的延迟工具（Deferred Tool），
/// 支持按需异步动态加载、延迟激活、元工具动态发现以及分发执行。
#[derive(Default)]
pub struct ToolRegistry {
    /// 当前已处于激活状态的工具（可随时供模型调用和执行）
    active_tools: HashMap<String, ActiveEntry>,
    /// 当前注册的延迟加载工具元信息
    deferred_tools: HashMap<String, DeferredEntry>,
}

impl ToolRegistry {
    /// 构造空工具注册中心
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个已具备完整 Schema 的标准活跃工具
    pub fn register_tool(
        &mut self,
        definition: FunctionTool,
        executor: impl ToolExecutor + 'static,
    ) {
        let name = definition.name.clone();
        self.active_tools.insert(
            name,
            ActiveEntry {
                definition,
                executor: Some(Arc::new(executor)),
            },
        );
    }

    /// 仅注册工具定义（无本地执行器，例如透传给远程微服务或由模型自行解析）
    pub fn register_definition_only(&mut self, definition: FunctionTool) {
        let name = definition.name.clone();
        self.active_tools.insert(
            name,
            ActiveEntry {
                definition,
                executor: None,
            },
        );
    }

    /// 注册延迟加载工具（附带专属动态加载器）
    pub fn register_deferred(
        &mut self,
        meta: DeferredToolMeta,
        loader: impl DeferredLoader + 'static,
    ) {
        let name = meta.name.clone();
        self.deferred_tools.insert(
            name,
            DeferredEntry {
                meta,
                loader: Arc::new(loader),
            },
        );
    }

    /// 检查工具是否已处于激活状态
    pub fn is_active(&self, name: &str) -> bool {
        self.active_tools.contains_key(name)
    }

    /// 检查工具是否为待加载的延迟工具
    pub fn is_deferred(&self, name: &str) -> bool {
        self.deferred_tools.contains_key(name)
    }

    /// 动态解析并激活指定延迟工具
    ///
    /// 若成功解析，将该工具注入活跃池中并返回完整定义，后续对话轮次模型可直接调用。
    pub async fn resolve_and_activate(
        &mut self,
        name: &str,
    ) -> Result<Option<FunctionTool>, AgentError> {
        // 1. 如果已经在活跃池中，直接返回
        if let Some(entry) = self.active_tools.get(name) {
            return Ok(Some(entry.definition.clone()));
        }

        // 2. 尝试使用专属延迟加载器加载
        if let Some(deferred_entry) = self.deferred_tools.remove(name) {
            let (definition, executor) = deferred_entry.loader.load(name).await?;
            self.active_tools.insert(
                name.to_string(),
                ActiveEntry {
                    definition: definition.clone(),
                    executor,
                },
            );
            return Ok(Some(definition));
        }

        Ok(None)
    }

    /// 获取当前所有已激活工具的完整定义列表（用于构建发送给模型的 CallOptions.tools）
    pub fn get_active_definitions(&self) -> Vec<FunctionTool> {
        self.active_tools
            .values()
            .map(|e| e.definition.clone())
            .collect()
    }

    /// 获取当前待加载的所有延迟工具元信息列表
    pub fn get_deferred_meta(&self) -> Vec<DeferredToolMeta> {
        self.deferred_tools
            .values()
            .map(|e| e.meta.clone())
            .collect()
    }

    /// 获取所有有效工具定义集合（已激活并符合 LLM 协议的工具）
    pub fn get_all_definitions(&self) -> Vec<ToolDefinition> {
        self.active_tools
            .values()
            .map(|e| ToolDefinition::Function(e.definition.clone()))
            .collect()
    }

    /// 执行指定工具（如果为延迟工具，则会自动触发动态激活后再行执行）
    pub async fn execute(&mut self, name: &str, input: Value) -> Result<ToolResultOutput, AgentError> {
        // 若尚未激活，先尝试动态解析并激活
        if !self.is_active(name) && self.is_deferred(name) {
            let _ = self.resolve_and_activate(name).await?;
        }

        let entry = self
            .active_tools
            .get(name)
            .ok_or_else(|| AgentError::ToolNotFound(name.to_string()))?;

        let executor = entry.executor.as_ref().ok_or_else(|| {
            AgentError::ToolExecutionFailed(
                name.to_string(),
                "该工具仅注册了定义，未配置执行器".to_string(),
            )
        })?;

        executor.execute(input).await
    }

    /// 构造用于供模型按需动态检索与加载工具的元工具（Meta-Tool）
    pub fn create_discovery_meta_tool(&self) -> FunctionTool {
        FunctionTool::new(
            "discover_deferred_tool",
            json!({
                "type": "object",
                "properties": {
                    "tool_name": {
                        "type": "string",
                        "description": "需要动态加载或查询详细 Schema 的工具名称"
                    }
                },
                "required": ["tool_name"]
            }),
        )
        .with_description("动态发现或检索延迟工具的完整定义与参数规范，调用后将该工具动态载入当前会话。")
    }
}
