//! 编译期内嵌的内置模型档案

use std::sync::OnceLock;

use super::ModelCatalog;

/// 内嵌数据文件 `(provider, json)` 清单，由 `build.rs` 扫描 `data/models/*.json` 生成
const SOURCES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin_sources.rs"));

pub(super) fn catalog() -> &'static ModelCatalog {
    static CATALOG: OnceLock<ModelCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut catalog = ModelCatalog::new();
        for (provider, json) in SOURCES {
            // 数据由 xtask 生成并经测试校验，解析失败属于构建产物损坏
            let parsed = ModelCatalog::from_json(json)
                .unwrap_or_else(|err| panic!("内置模型档案 `{provider}` 解析失败: {err}"));
            catalog.extend(parsed);
        }
        catalog
    })
}
