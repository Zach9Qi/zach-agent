//! 模型档案目录：按 `(provider, id)` 查询 [`ModelProfile`]
//!
//! 内置数据来自 models.dev，经 `cargo xtask sync-models` 生成到 `data/models/` 并随源码提交；
//! 调用方可在内置目录基础上覆盖或追加自定义档案。

mod builtin;

use std::collections::HashMap;

use zach_ai_core::ModelProfile;

/// 模型档案目录
#[derive(Debug, Clone, Default)]
pub struct ModelCatalog {
    profiles: HashMap<(String, String), ModelProfile>,
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
        let mut catalog = Self::new();
        catalog.extend(profiles);
        Ok(catalog)
    }

    /// 按厂商与模型标识查询
    pub fn get(&self, provider: &str, id: &str) -> Option<&ModelProfile> {
        self.profiles.get(&(provider.to_owned(), id.to_owned()))
    }

    /// 列出某厂商的全部模型，按模型标识排序
    pub fn provider_models(&self, provider: &str) -> Vec<&ModelProfile> {
        let mut models: Vec<&ModelProfile> = self
            .profiles
            .values()
            .filter(|profile| profile.provider == provider)
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models
    }

    /// 遍历全部档案（无序）
    pub fn iter(&self) -> impl Iterator<Item = &ModelProfile> {
        self.profiles.values()
    }

    /// 档案数量
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// 插入档案，同 `(provider, id)` 的已有档案会被覆盖并返回
    pub fn insert(&mut self, profile: ModelProfile) -> Option<ModelProfile> {
        let key = (profile.provider.clone(), profile.id.clone());
        self.profiles.insert(key, profile)
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
