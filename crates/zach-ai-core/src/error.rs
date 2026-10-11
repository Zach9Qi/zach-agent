//! 统一错误体系
//!
//! - [`ErrorKind`]：按"调用方该怎么处理"划分的类别，是可重试性等一切处理决策的唯一依据；
//! - [`ModelError`]：运行时错误，由类别、统一的上下文字段与 `source` 链组成；
//! - [`ErrorInfo`]：可序列化投影，跨越事件、流、持久化边界时只传它；
//! - [`report`]：把错误及其 `source` 链渲染成一行。
//!
//! 约定：各层 `Display` 只描述本层事实，不拼接 `source`；需要完整原因时用 [`report`]，
//! 因此逐层包装不会出现重复的前缀。

mod info;
mod kind;
mod model;
mod report;

pub use info::ErrorInfo;
pub use kind::ErrorKind;
pub use model::ModelError;
pub use report::report;
