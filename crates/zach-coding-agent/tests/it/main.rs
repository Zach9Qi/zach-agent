//! zach-coding-agent 集成测试：单二进制入口，各子模块按被测主题划分
//!
//! 只通过 crate 公开 API 验证对外契约；共享测试替身（内存文件系统等）位于 `support`。

mod support;

mod edit;
mod local_fs;
mod local_shell;
mod policy;
mod read;
mod truncate;
mod workspace;
mod write;
