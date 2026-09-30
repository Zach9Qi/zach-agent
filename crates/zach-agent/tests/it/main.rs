//! zach-agent 集成测试：单二进制入口，各子模块按被测主题划分
//!
//! 只通过 crate 公开 API 验证对外契约；共享测试替身位于 `support`。

mod support;

mod agent;
mod agent_approval;
mod agent_backpressure;
mod agent_loop;
mod agent_start;
mod agent_stream_error;
mod config;
mod event_mapping;
mod event_serde;
mod tool;
