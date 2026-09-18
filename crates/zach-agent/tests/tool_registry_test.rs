//! ToolRegistry 与延迟加载工具（Deferred Tool）调度集成测试

use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use zach_agent::{
    AsyncDeferredLoader, AsyncFnExecutor, DeferredToolMeta, SyncFnExecutor, ToolRegistry,
};
use zach_ai_core::{FunctionTool, ToolResultOutput};

#[tokio::test]
async fn test_active_tool_registration_and_execution() {
    let mut registry = ToolRegistry::new();

    let tool_def = FunctionTool::new(
        "add",
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "number" }
            },
            "required": ["a", "b"]
        }),
    )
    .with_description("计算两数之和");

    registry.register_tool(
        tool_def,
        SyncFnExecutor::new(|input: serde_json::Value| {
            let a = input["a"].as_f64().unwrap_or(0.0);
            let b = input["b"].as_f64().unwrap_or(0.0);
            Ok(ToolResultOutput::json(json!({ "sum": a + b })))
        }),
    );

    assert!(registry.is_active("add"));
    assert!(!registry.is_deferred("add"));

    let res = registry
        .execute("add", json!({ "a": 10, "b": 25 }))
        .await
        .unwrap();

    match res {
        ToolResultOutput::Json { value, .. } => {
            assert_eq!(value["sum"], 35.0);
        }
        _ => panic!("期望返回 JSON 结果"),
    }
}

#[tokio::test]
async fn test_deferred_tool_dynamic_loading_and_activation() {
    let mut registry = ToolRegistry::new();
    let load_count = Arc::new(AtomicUsize::new(0));

    let deferred = DeferredToolMeta::new("search_database")
        .with_description("按关键字搜索数据库记录")
        .with_namespace("db_service");

    let count_clone = load_count.clone();
    registry.register_deferred(
        deferred,
        AsyncDeferredLoader::new(move |name: &str| {
            let count = count_clone.clone();
            let name_str = name.to_string();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                let tool = FunctionTool::new(
                    name_str,
                    json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string" }
                        },
                        "required": ["query"]
                    }),
                )
                .with_description("已动态加载的数据库搜索工具");

                let executor: Arc<dyn zach_agent::ToolExecutor> =
                    Arc::new(AsyncFnExecutor::new(|input: serde_json::Value| async move {
                        let query = input["query"].as_str().unwrap_or("");
                        Ok(ToolResultOutput::text(format!("找到关于 '{query}' 的 3 条记录")))
                    }));

                Ok((tool, Some(executor)))
            }
        }),
    );

    // 初始状态应在 deferred 中，且尚未加载
    assert!(!registry.is_active("search_database"));
    assert!(registry.is_deferred("search_database"));
    assert_eq!(load_count.load(Ordering::SeqCst), 0);

    // 验证获取待加载元数据列表
    let deferred_metas = registry.get_deferred_meta();
    assert_eq!(deferred_metas.len(), 1);
    assert_eq!(deferred_metas[0].name, "search_database");

    // 模拟调用：自动触发动态加载与激活
    let result = registry
        .execute("search_database", json!({ "query": "Rust Agent" }))
        .await
        .unwrap();

    match result {
        ToolResultOutput::Text { value, .. } => {
            assert_eq!(value, "找到关于 'Rust Agent' 的 3 条记录");
        }
        _ => panic!("期望返回 Text 结果"),
    }

    // 再次检查状态：此时已被激活并缓存，不需要重复加载
    assert!(registry.is_active("search_database"));
    assert!(!registry.is_deferred("search_database"));
    assert_eq!(load_count.load(Ordering::SeqCst), 1);

    // 激活后在所有已激活定义中可见
    let all_defs = registry.get_all_definitions();
    assert_eq!(all_defs.len(), 1);
    assert_eq!(all_defs[0].name(), "search_database");

    // 再次执行，验证走缓存
    let _ = registry
        .execute("search_database", json!({ "query": "Tokio" }))
        .await
        .unwrap();
    assert_eq!(load_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_tool_registry_discovery_meta_tool() {
    let registry = ToolRegistry::new();

    // 验证生成延迟发现元工具
    let meta_tool = registry.create_discovery_meta_tool();
    assert_eq!(meta_tool.name, "discover_deferred_tool");
    assert!(meta_tool.description.is_some());
}
