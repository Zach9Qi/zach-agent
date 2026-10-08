//! 联调输出的 Rust 类型；修改后运行 cargo xtask probe-schemas 更新共享 Schema。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 混合附件的逐项读取结果及汇总。
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AttachmentSummary {
    /// 第一张图片中的数值。
    image_a: i64,
    /// 第二张图片中的数值。
    image_b: i64,
    /// 第一份报告中的数值。
    report_a: i64,
    /// 第二份报告中的数值。
    report_b: i64,
    /// 全部附件数值之和。
    total: i64,
    /// 用户正文提供的校验码。
    text_code: String,
}
