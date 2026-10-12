//! read 工具：分段读取文本文件，或以附件形式读取图片

use super::text::{is_binary, strip_bom};
use super::ToolEnv;
use crate::truncate::truncate_head;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};
use zach_ai_core::{FileData, ToolResultContentBlock, ToolResultOutput};

/// 单张图片的大小上限（主流厂商的单图限制）
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ReadInput {
    /// 文件路径，相对工作区根目录或绝对路径
    path: String,
    /// 起始行号，从 1 开始，默认 1
    #[serde(default)]
    offset: Option<usize>,
    /// 最多读取的行数，默认读到截断上限为止
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) struct ReadTool {
    env: ToolEnv,
    description: String,
}

impl ReadTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        let description = format!(
            "读取文件内容。支持文本文件与图片（png、jpg、gif、webp），图片以附件形式返回。\
             文本输出最多 {} 行或 {}KB（先到者为准），超出部分用 offset/limit 分段读取；\
             需要全文时持续增大 offset 直到读完。",
            env.limits.max_lines,
            env.limits.max_bytes / 1024
        );
        Self { env, description }
    }

    fn read_image(&self, path: &Path, media_type: &str, bytes: Vec<u8>) -> ToolOutcome {
        let display = self.env.display(path);
        let size = bytes.len();
        let blocks = vec![
            ToolResultContentBlock::Text {
                text: format!("图片 {display}（{media_type}，{size} 字节）"),
                provider_options: None,
            },
            ToolResultContentBlock::File {
                media_type: media_type.to_string(),
                data: FileData::from_bytes(bytes),
                filename: path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned()),
                provider_options: None,
            },
        ];
        ToolOutcome::new(ToolResultOutput::Content { value: blocks }).with_details(json!({
            "path": display,
            "media_type": media_type,
            "bytes": size,
        }))
    }

    fn read_text(&self, path: &Path, bytes: &[u8], input: &ReadInput) -> ToolOutcome {
        let display = self.env.display(path);
        let decoded = String::from_utf8_lossy(bytes);
        let (text, _) = strip_bom(&decoded);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        if total == 0 {
            return ToolOutcome::text(format!("（{display} 是空文件）"))
                .with_details(json!({ "path": display, "total_lines": 0 }));
        }

        let start = input.offset.unwrap_or(1).max(1);
        if start > total {
            return ToolOutcome::error(format!(
                "offset {start} 超出文件末尾（{display} 共 {total} 行）"
            ));
        }
        let end = input
            .limit
            .map_or(total, |limit| (start - 1 + limit).min(total));
        let selected = lines[start - 1..end.max(start)].join("\n");
        let truncation = truncate_head(&selected, self.env.limits);

        if truncation.output_lines == 0 {
            let size = lines[start - 1].len();
            return ToolOutcome::text(format!(
                "[第 {start} 行有 {size} 字节，超过 {} 字节的输出上限，无法显示。\
                 可改用命令截取该行的一部分]",
                self.env.limits.max_bytes
            ))
            .with_details(
                json!({ "path": display, "total_lines": total, "truncation": truncation }),
            );
        }

        let last = start + truncation.output_lines - 1;
        let mut output = truncation.content.clone();
        if truncation.is_truncated() {
            output.push_str(&format!(
                "\n\n[显示第 {start}-{last} 行，共 {total} 行。使用 offset={} 继续读取]",
                last + 1
            ));
        } else if last < total {
            output.push_str(&format!(
                "\n\n[还有 {} 行未显示。使用 offset={} 继续读取]",
                total - last,
                last + 1
            ));
        }
        let mut details = json!({
            "path": display,
            "total_lines": total,
            "start_line": start,
            "end_line": last,
        });
        if truncation.is_truncated() {
            details["truncation"] = json!(truncation);
        }
        ToolOutcome::text(output).with_details(details)
    }
}

#[async_trait]
impl TypedTool for ReadTool {
    type Input = ReadInput;

    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn needs_approval(&self, input: &ReadInput) -> bool {
        self.env.needs_approval(&input.path, false)
    }

    async fn call(&self, input: ReadInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        let path = self.env.resolve(&input.path, false)?;
        let info = self
            .env
            .fs
            .metadata(&path)
            .await
            .map_err(|error| self.env.io_error(error, &path))?;
        if info.is_dir {
            return Err(ToolError::failed(format!(
                "{} 是目录，请用 ls 查看其内容",
                self.env.display(&path)
            )));
        }
        let bytes = self
            .env
            .fs
            .read(&path)
            .await
            .map_err(|error| self.env.io_error(error, &path))?;

        if let Some(media_type) = image_type(&bytes) {
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err(ToolError::failed(format!(
                    "图片 {} 有 {} 字节，超过 {MAX_IMAGE_BYTES} 字节的上限",
                    self.env.display(&path),
                    bytes.len()
                )));
            }
            return Ok(self.read_image(&path, media_type, bytes));
        }
        if is_binary(&bytes) {
            return Err(ToolError::failed(format!(
                "{} 是二进制文件，无法以文本读取",
                self.env.display(&path)
            )));
        }
        Ok(self.read_text(&path, &bytes, &input))
    }
}

/// 按文件头魔数识别图片类型
fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}
