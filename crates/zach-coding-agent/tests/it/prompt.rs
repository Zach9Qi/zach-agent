//! 系统提示词：项目指令文件的发现顺序与取舍，以及各段落的渲染

use crate::support::{at, root, tools, MemoryFs};
use std::path::PathBuf;
use zach_coding_agent::{ContextFile, ContextFiles, SystemPrompt, Workspace};

/// 工作区根目录的上一级
fn parent() -> PathBuf {
    root().parent().unwrap().to_path_buf()
}

fn file(path: PathBuf, content: &str) -> ContextFile {
    ContextFile {
        path,
        content: content.into(),
    }
}

#[tokio::test]
async fn instruction_files_run_from_the_global_directory_to_the_workspace_root() {
    let fs = MemoryFs::new();
    let global = parent().join("home");
    fs.put(&global.join("AGENTS.md"), "全局偏好\n");
    fs.put(&parent().join("CLAUDE.md"), "上级约定");
    fs.put(&at("AGENTS.md"), "项目约定\n\n");
    fs.put(&at("sub/AGENTS.md"), "子目录约定不在查找范围内");

    let files = ContextFiles::new()
        .with_global_dir(&global)
        .discover(fs.as_ref(), &root())
        .await;
    assert_eq!(
        files,
        [
            file(global.join("AGENTS.md"), "全局偏好"),
            file(parent().join("CLAUDE.md"), "上级约定"),
            file(at("AGENTS.md"), "项目约定"),
        ]
    );
}

#[tokio::test]
async fn each_directory_contributes_its_first_non_blank_file() {
    let fs = MemoryFs::new();
    fs.put(&at("AGENTS.md"), " \n");
    fs.put(&at("CLAUDE.md"), "\u{feff}用 CLAUDE.md\n");
    fs.put(&at("RULES.md"), "自定义文件名");
    fs.put(&parent().join("AGENTS.md"), "上级");

    let files = ContextFiles::new().discover(fs.as_ref(), &root()).await;
    assert_eq!(
        files,
        [
            file(parent().join("AGENTS.md"), "上级"),
            file(at("CLAUDE.md"), "用 CLAUDE.md"),
        ]
    );

    let custom = ContextFiles::new()
        .with_names(["RULES.md"])
        .discover(fs.as_ref(), &root())
        .await;
    assert_eq!(custom, [file(at("RULES.md"), "自定义文件名")]);
}

/// 全局目录恰好是工作区根目录（或其祖先）时，同一文件只出现一次
#[tokio::test]
async fn a_global_directory_on_the_walk_is_not_repeated() {
    let fs = MemoryFs::new();
    fs.put(&at("AGENTS.md"), "项目约定");
    let files = ContextFiles::new()
        .with_global_dir(root())
        .discover(fs.as_ref(), &root())
        .await;
    assert_eq!(files, [file(at("AGENTS.md"), "项目约定")]);
}

/// 完整渲染结果：固定各段顺序与格式
#[test]
fn sections_render_in_a_fixed_order() {
    let prompt = SystemPrompt::new(&Workspace::new(root()))
        .with_preamble("你是测试助手")
        .with_rule("先写测试")
        .with_rule(" 先写测试 ")
        .with_rule("")
        .with_addendum("附加说明\n")
        .with_context_files(vec![
            file(parent().join("AGENTS.md"), "上级"),
            file(at("AGENTS.md"), "用中文"),
        ])
        .render();
    let expected = format!(
        "你是测试助手\n\n\
         <rules>\n- 先写测试\n- 回答简洁直接\n- 提到文件时写出清晰的路径\n</rules>\n\n\
         <addendum>\n附加说明\n</addendum>\n\n\
         <project_context>\n\
         以下是项目中的指令与约定，必须遵守。越靠后的文件来自越靠近工作区的目录，内容冲突时以靠后的为准。\n\n\
         <project_instructions path=\"{}\">\n上级\n</project_instructions>\n\n\
         <project_instructions path=\"AGENTS.md\">\n用中文\n</project_instructions>\n\
         </project_context>\n\n\
         <workspace>\n工作区根目录：{}\n相对路径以工作区根目录为基准。\n</workspace>",
        parent().join("AGENTS.md").display(),
        root().display(),
    );
    assert_eq!(prompt, expected);
}

#[test]
fn rules_follow_the_tools_handed_to_the_agent() {
    let fs = MemoryFs::new();
    let workspace = Workspace::new(root());
    let coding = SystemPrompt::new(&workspace)
        .with_tools(&tools(&fs).coding())
        .render();
    assert!(coding.starts_with("你是一名资深的编码助手"));
    assert!(coding.contains("- 修改已有文件前先用 read 查看要改动的部分"));
    assert!(coding.contains("- 修改已有文件用 edit 做精确替换"));
    assert!(!coding.contains("<addendum>") && !coding.contains("<project_context>"));

    let read_only = SystemPrompt::new(&workspace)
        .with_tools(&tools(&fs).read_only())
        .render();
    assert!(!read_only.contains("edit") && !read_only.contains("write"));
}
