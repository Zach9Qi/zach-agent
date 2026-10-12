//! 标准请求中未被识别字段的检查
//!
//! `serde_ignored` 看不到按标签区分的枚举（消息、内容块、输出格式等）内部的字段：这类枚举会先把
//! 整个对象缓冲下来再反序列化，缓冲之后被丢弃的字段不会触发回调。因此改为把反序列化结果再序列化
//! 回 JSON，与原始请求逐层比对：原始对象中有、往返结果中没有的键，就是标准类型不识别的字段。

use serde_json::Value;

/// 返回 `original` 中第一个未被标准类型识别的字段路径
pub(super) fn unknown(original: &Value, round_trip: &Value) -> Option<String> {
    walk(original, round_trip, "request")
}

fn walk(original: &Value, round_trip: &Value, path: &str) -> Option<String> {
    match (original, round_trip) {
        (Value::Object(original), Value::Object(round_trip)) => {
            original.iter().find_map(|(key, value)| {
                let path = format!("{path}.{key}");
                match round_trip.get(key) {
                    Some(next) => walk(value, next, &path),
                    // 取默认值（None、空列表）的字段序列化时会被省略
                    None if is_default(value) => None,
                    None => Some(path),
                }
            })
        }
        (Value::Array(original), Value::Array(round_trip)) => original
            .iter()
            .zip(round_trip)
            .enumerate()
            .find_map(|(index, (value, next))| walk(value, next, &format!("{path}[{index}]"))),
        _ => None,
    }
}

fn is_default(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}
