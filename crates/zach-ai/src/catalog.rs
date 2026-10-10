//! 模型档案目录：按 `(provider, id)` 查询 [`ModelProfile`]
//!
//! 内置数据来自 models.dev，经 `cargo xtask sync-models` 生成到 `data/models/` 并随源码提交；
//! 调用方可在内置目录基础上覆盖或追加自定义档案。

mod builtin;

use std::collections::HashMap;

use zach_ai_core::ModelProfile;

/// 模型档案目录
///
/// 按厂商、模型标识两级索引，查询只做哈希查找，不分配临时字符串。
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    providers: HashMap<String, HashMap<String, ModelProfile>>,
}

impl ModelCatalog {
    /// 空目录
    pub fn new() -> Self {
        Self::default()
    }

    /// 内置目录的全局共享实例
    pub fn builtin() -> &'static ModelCatalog {
        builtin::catalog()
    }

    /// 从 `ModelProfile` 数组形式的 JSON 构建目录
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let profiles: Vec<ModelProfile> = serde_json::from_str(json)?;
        Ok(profiles.into_iter().collect())
    }

    /// 按厂商与模型标识查询
    pub fn get(&self, provider: &str, id: &str) -> Option<&ModelProfile> {
        self.providers.get(provider)?.get(id)
    }

    /// 列出某厂商的全部模型，按模型标识排序
    pub fn provider_models(&self, provider: &str) -> Vec<&ModelProfile> {
        let mut models: Vec<&ModelProfile> = self
            .providers
            .get(provider)
            .map(|models| models.values().collect())
            .unwrap_or_default();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models
    }

    /// 遍历全部档案（无序）
    pub fn iter(&self) -> impl Iterator<Item = &ModelProfile> {
        self.providers.values().flat_map(HashMap::values)
    }

    /// 档案数量
    pub fn len(&self) -> usize {
        self.providers.values().map(HashMap::len).sum()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.providers.values().all(HashMap::is_empty)
    }

    /// 插入档案，同 `(provider, id)` 的已有档案会被覆盖并返回
    pub fn insert(&mut self, profile: ModelProfile) -> Option<ModelProfile> {
        self.providers
            .entry(profile.provider.clone())
            .or_default()
            .insert(profile.id.clone(), profile)
    }
}

impl Extend<ModelProfile> for ModelCatalog {
    fn extend<T: IntoIterator<Item = ModelProfile>>(&mut self, iter: T) {
        for profile in iter {
            self.insert(profile);
        }
    }
}

impl FromIterator<ModelProfile> for ModelCatalog {
    fn from_iter<T: IntoIterator<Item = ModelProfile>>(iter: T) -> Self {
        let mut catalog = Self::new();
        catalog.extend(iter);
        catalog
    }
}

impl IntoIterator for ModelCatalog {
    type Item = ModelProfile;
    type IntoIter = std::vec::IntoIter<ModelProfile>;

    /// 按档案逐个取出，顺序不保证
    fn into_iter(self) -> Self::IntoIter {
        self.providers
            .into_values()
            .flat_map(HashMap::into_values)
            .collect::<Vec<_>>()
            .into_iter()
    }
}
