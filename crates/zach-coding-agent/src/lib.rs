//! 编码智能体运行时：在 `zach-agent` 之上提供工作区感知的文件与命令工具
//!
//! 本 crate 只是运行时库，不含命令行与界面：模型、配置与审批交互都由宿主注入，
//! 运行过程通过 `zach-agent` 的事件流透出。

mod truncate;
mod workspace;

pub use truncate::{truncate_head, truncate_tail, TruncatedBy, Truncation, TruncationLimits};
pub use workspace::{OutsideAccess, Workspace};
