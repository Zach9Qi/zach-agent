//! 加载共享输出 Schema，保留标准格式的名称与说明，并拒绝多处定义。

use super::super::ProbeResult;
use serde_json::{json, Value};
use std::path::Path;
use zach_ai_core::{CallOptions, ResponseFormat};

pub(super) fn prepare(request: &mut Value, schema_file: &Path) -> ProbeResult<()> {
    if schema_file.as_os_str().is_empty() {
        return Err("schema_file 不能为空".into());
    }
    let request = request.as_object_mut().ok_or("标准 request 必须为对象")?;
    let format = request
        .entry("response_format")
        .or_insert_with(|| json!({"type":"json"}));
    if format.get("type").and_then(Value::as_str) != Some("json") {
        return Err("schema_file 仅适用于 JSON 输出，不能同时配置其他 response_format".into());
    }
    if format.get("schema").is_some() {
        return Err("schema_file 与 request.response_format.schema 不能同时提供".into());
    }
    Ok(())
}

pub(super) fn bind(
    request: &mut CallOptions,
    schema_file: &Path,
    scenario_path: &Path,
    read: &mut impl FnMut(&Path) -> ProbeResult<Vec<u8>>,
) -> ProbeResult<()> {
    let path = scenario_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(schema_file);
    let bytes =
        read(&path).map_err(|e| format!("读取 Schema 文件 {} 失败: {e}", path.display()))?;
    let source = std::str::from_utf8(&bytes).map_err(|_| "Schema 文件必须使用 UTF-8 编码")?;
    let schema: Value =
        serde_json::from_str(source.trim_start_matches('\u{feff}')).map_err(|e| {
            // 只输出位置，不将 Schema 文件中的原始值带入错误日志。
            format!(
                "Schema 文件 JSON 格式无效（第 {} 行，第 {} 列）",
                e.line(),
                e.column()
            )
        })?;
    if !schema.is_object() {
        return Err("Schema 文件必须包含一个完整的 JSON Schema 对象".into());
    }
    let Some(ResponseFormat::Json { schema: target, .. }) = &mut request.response_format else {
        return Err("Schema 加载前必须指定 JSON 输出".into());
    };
    *target = Some(schema);
    Ok(())
}
