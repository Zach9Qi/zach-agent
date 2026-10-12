//! edit 工具：对已有文件做一处或多处精确替换
//!
//! 换行符与 BOM 原样往返：匹配在 LF 空间进行，写回时恢复原文件的 CRLF 与 BOM。
//! 模型只收到一句简短结论，统一差异放进结果详情供宿主展示。

mod apply;
mod fuzzy;

use super::text::{is_binary, strip_bom};
use super::ToolEnv;
use apply::{apply, Replacement};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer};
use serde_json::json;
use similar::{DiffTag, TextDiff};
use zach_agent::{ToolContext, ToolError, ToolOutcome, TypedTool};

/// 差异中每处改动前后保留的上下文行数
const DIFF_CONTEXT: usize = 3;

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EditInput {
    /// 文件路径，相对工作区根目录或绝对路径
    path: String,
    /// 要做的替换。每一项都对照原文件定位（而不是在前一项替换后的结果上），彼此不能重叠
    #[serde(deserialize_with = "lenient_edits")]
    #[schemars(with = "Vec<EditOp>")]
    edits: Vec<EditOp>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EditOp {
    /// 要替换的原文：必须与文件内容完全一致（包括缩进、空白与换行），且在文件中唯一
    old_text: String,
    /// 替换后的文本
    new_text: String,
}

pub(crate) struct EditTool {
    env: ToolEnv,
}

impl EditTool {
    pub(crate) fn new(env: ToolEnv) -> Self {
        Self { env }
    }
}

#[async_trait]
impl TypedTool for EditTool {
    type Input = EditInput;

    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "对已有文件做精确的文本替换。edits 中每一项的 old_text 必须与文件内容完全一致\
         （包括缩进、空白与换行）且在文件中唯一；old_text 尽量短，能唯一定位即可。\
         所有替换都对照原文件定位、互不重叠：同一处或相邻几行的改动请合并为一项。\
         新建文件或整体重写请用 write。"
    }

    fn needs_approval(&self, input: &EditInput) -> bool {
        self.env.needs_approval(&input.path, true)
    }

    async fn call(&self, input: EditInput, _ctx: ToolContext) -> Result<ToolOutcome, ToolError> {
        if input.edits.is_empty() {
            return Err(ToolError::invalid_input("edits 不能为空"));
        }
        let path = self.env.resolve(&input.path, true)?;
        let display = self.env.display(&path);
        let _guard = self.env.locks.lock(&path).await;

        let bytes = self
            .env
            .fs
            .read(&path)
            .await
            .map_err(|error| self.env.io_error(error, &path))?;
        if is_binary(&bytes) {
            return Err(ToolError::failed(format!(
                "{display} 是二进制文件，无法编辑"
            )));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| ToolError::failed(format!("{display} 不是有效的 UTF-8 文本，无法编辑")))?;
        let (body, bom) = strip_bom(&text);
        let crlf = uses_crlf(body);
        let original = body.replace("\r\n", "\n");

        let edits: Vec<(String, String)> = input
            .edits
            .iter()
            .map(|op| {
                (
                    op.old_text.replace("\r\n", "\n"),
                    op.new_text.replace("\r\n", "\n"),
                )
            })
            .collect();
        let replacements: Vec<Replacement<'_>> = edits
            .iter()
            .map(|(old_text, new_text)| Replacement { old_text, new_text })
            .collect();
        let applied = apply(&original, &replacements)
            .map_err(|error| ToolError::failed(format!("{display}: {error}")))?;

        let mut output = if crlf {
            applied.content.replace('\n', "\r\n")
        } else {
            applied.content.clone()
        };
        if bom {
            output.insert(0, '\u{feff}');
        }
        self.env
            .fs
            .write(&path, output.as_bytes())
            .await
            .map_err(|error| self.env.io_error(error, &path))?;

        let diff = TextDiff::from_lines(original.as_str(), applied.content.as_str());
        let first_changed_line = diff
            .ops()
            .iter()
            .find(|op| op.tag() != DiffTag::Equal)
            .map(|op| op.new_range().start + 1);
        let unified = diff
            .unified_diff()
            .context_radius(DIFF_CONTEXT)
            .header(&display, &display)
            .to_string();

        let count = replacements.len();
        let mut summary = format!("已在 {display} 中替换 {count} 处");
        if applied.fuzzy > 0 {
            summary.push_str(&format!(
                "（其中 {} 处为宽松匹配：忽略了行尾空白及引号、破折号、空格的 Unicode 变体）",
                applied.fuzzy
            ));
        }
        Ok(ToolOutcome::text(summary).with_details(json!({
            "path": display,
            "replacements": count,
            "fuzzy_matches": applied.fuzzy,
            "first_changed_line": first_changed_line,
            "diff": unified,
        })))
    }
}

/// 以第一个换行符的形式为准判断是否使用 CRLF
fn uses_crlf(text: &str) -> bool {
    text.find('\n')
        .is_some_and(|index| text[..index].ends_with('\r'))
}

/// 兼容模型常见的入参变形：单个对象、被编码成 JSON 字符串的数组或对象
fn lenient_edits<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<EditOp>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Many(Vec<EditOp>),
        One(EditOp),
        Encoded(String),
    }

    fn flatten<E: serde::de::Error>(shape: Shape, nested: bool) -> Result<Vec<EditOp>, E> {
        match shape {
            Shape::Many(edits) => Ok(edits),
            Shape::One(edit) => Ok(vec![edit]),
            Shape::Encoded(text) if !nested => serde_json::from_str::<Shape>(&text)
                .map_err(|_| E::custom("edits 应为 {old_text, new_text} 对象数组"))
                .and_then(|shape| flatten(shape, true)),
            Shape::Encoded(_) => Err(E::custom("edits 应为 {old_text, new_text} 对象数组")),
        }
    }

    let shape = Shape::deserialize(deserializer)
        .map_err(|_| serde::de::Error::custom("edits 应为 {old_text, new_text} 对象数组"))?;
    flatten(shape, false)
}
