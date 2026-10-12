//! write 工具：新建文件或整体覆盖

use super::ToolEnv;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct WriteInput {
    /// 文件路径，相对工作区根目录或绝对路径
    path: String,
    /// 完整的文件内容
    content: String,
}

pub(crate) struct WriteTool {
    env: ToolEnv,
}

impl WriteTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        Self { env }
    }
}

#[async_trait]
impl TypedTool for WriteTool {
    type Input = WriteInput;

    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "写入文件：不存在则创建（自动创建父目录），已存在则整体覆盖。\
         只用于新建文件或整体重写；修改已有文件的局部内容请用 edit。"
    }

    fn needs_approval(&self, input: &WriteInput) -> bool {
        self.env.needs_approval(&input.path, true)
    }

    async fn call(&self, input: WriteInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let path = self.env.resolve(&input.path, true)?;
        let display = self.env.display(&path);
        let _guard = self.env.locks.lock(&path).await;

        let existed = match self.env.fs.metadata(&path).await {
            Ok(info) if info.is_dir => {
                return Err(ToolError::failed(format!("{display} 是目录，无法写入")));
            }
            Ok(_) => true,
            Err(_) => false,
        };
        if let Some(parent) = path.parent() {
            self.env
                .fs
                .create_dir_all(parent)
                .await
                .map_err(|error| self.env.io_error(error, parent))?;
        }
        let bytes = input.content.len();
        self.env
            .fs
            .write(&path, input.content.as_bytes())
            .await
            .map_err(|error| self.env.io_error(error, &path))?;

        let verb = if existed { "覆盖" } else { "新建" };
        Ok(
            ToolOutcome::text(format!("已{verb} {display}（{bytes} 字节）")).with_details(json!({
                "path": display,
                "bytes": bytes,
                "created": !existed,
            })),
        )
    }
}
