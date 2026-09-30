//! zach-ai-core 集成测试：单二进制入口，各子模块按被测主题划分
//!
//! 只通过 crate 公开 API 验证对外契约。

mod accumulator;
mod accumulator_error;
mod error;
mod function_tool;
mod model_profile;
mod provider_tool;
mod serde_naming;
mod tool_input;
mod tool_result;
mod usage;
mod v4_roundtrip;
