//! 从 models.dev 拉取模型数据，转换为 `ModelProfile` 后写入 `crates/zach-ai/data/models/`

mod convert;
mod source;

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use zach_ai_core::ModelProfile;

use source::ModelsDevCatalog;

const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// 需要内置的厂商（models.dev 中的 provider 标识）
///
/// 这是唯一的手工维护清单：`zach-ai` 的 `build.rs` 会自动嵌入 `data/models/` 下的全部文件，
/// 因此增删厂商只需修改此处并重新运行 `cargo xtask sync-models`。
pub(crate) const PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "deepseek",
    "xai",
    "mistral",
    "moonshotai",
    "zhipuai",
    "alibaba",
    "minimax",
];

pub(crate) fn run() -> Result<(), Box<dyn Error>> {
    println!("拉取 {MODELS_DEV_URL} ...");
    let catalog: ModelsDevCatalog = reqwest::blocking::Client::new()
        .get(MODELS_DEV_URL)
        .send()?
        .error_for_status()?
        .json()?;

    let out_dir = output_dir();
    fs::create_dir_all(&out_dir)?;

    for &provider in PROVIDERS {
        let Some(source) = catalog.get(provider) else {
            return Err(format!("models.dev 中缺少厂商 `{provider}`").into());
        };
        // BTreeMap 保证按模型 id 排序，使每次同步的 diff 稳定
        let profiles: BTreeMap<&str, ModelProfile> = source
            .models
            .iter()
            .filter(|(_, model)| model.tool_call)
            .map(|(id, model)| (id.as_str(), convert::to_profile(provider, id, model)))
            .collect();
        let profiles: Vec<ModelProfile> = profiles.into_values().collect();

        let path = out_dir.join(format!("{provider}.json"));
        let mut json = serde_json::to_string_pretty(&profiles)?;
        json.push('\n');
        fs::write(&path, json)?;
        println!("  {provider}: {} 个模型", profiles.len());
    }
    remove_stale_files(&out_dir)?;
    Ok(())
}

/// 删除不在 `PROVIDERS` 中的数据文件，保证目录内容与清单严格一致
fn remove_stale_files(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(out_dir)? {
        let path = entry?.path();
        let is_json = path.extension().is_some_and(|ext| ext == "json");
        let provider = path.file_stem().and_then(|stem| stem.to_str());
        if is_json && provider.is_some_and(|p| !PROVIDERS.contains(&p)) {
            fs::remove_file(&path)?;
            println!("  已删除陈旧文件 {}", path.display());
        }
    }
    Ok(())
}

fn output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates/zach-ai/data/models")
}
