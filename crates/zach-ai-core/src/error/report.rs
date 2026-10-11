//! 错误链渲染

use std::error::Error;

/// 把错误及其 `source` 链渲染成一行：`顶层: 原因: 更深的原因`
///
/// 各层 `Display` 只描述本层事实，完整原因只在这里拼接，因此不会重复。
/// 适用于任意实现了 [`Error`] 的类型，日志、事件文本、回传给模型的工具错误都应经由此处。
pub fn report(error: &dyn Error) -> String {
    let mut text = error.to_string();
    for cause in std::iter::successors(error.source(), |&cause| cause.source()) {
        text.push_str(": ");
        text.push_str(&cause.to_string());
    }
    text
}
