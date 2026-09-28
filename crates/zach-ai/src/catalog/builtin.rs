//! 编译期内嵌的内置模型档案

use std::sync::OnceLock;

use super::ModelCatalog;

/// 内嵌数据文件；与 `xtask/src/sync_models.rs` 中的 `PROVIDERS` 保持一致
const SOURCES: &[(&str, &str)] = &[
    ("anthropic", include_str!("../../data/models/anthropic.json")),
    ("openai", include_str!("../../data/models/openai.json")),
    ("google", include_str!("../../data/models/google.json")),
    ("deepseek", include_str!("../../data/models/deepseek.json")),
    ("xai", include_str!("../../data/models/xai.json")),
    ("mistral", include_str!("../../data/models/mistral.json")),
    ("moonshotai", include_str!("../../data/models/moonshotai.json")),
    ("zhipuai", include_str!("../../data/models/zhipuai.json")),
    ("alibaba", include_str!("../../data/models/alibaba.json")),
    ("minimax", include_str!("../../data/models/minimax.json")),
];

pub(super) fn catalog() -> &'static ModelCatalog {
    static CATALOG: OnceLock<ModelCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut catalog = ModelCatalog::new();
        for (provider, json) in SOURCES {
            // 数据由 xtask 生成并经测试校验，解析失败属于构建产物损坏
            let parsed = ModelCatalog::from_json(json)
                .unwrap_or_else(|err| panic!("内置模型档案 `{provider}` 解析失败: {err}"));
            catalog.extend(parsed.profiles.into_values());
        }
        catalog
    })
}
