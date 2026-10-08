//! 内置场景及真实附件在编译时嵌入，防止文档示例与加载器格式漂移。

use super::super::{config::Mode, scenario::Scenario};
use std::path::Path;

#[test]
fn every_bundled_scenario_loads_with_its_committed_files() {
    let all = [Mode::Generate, Mode::Stream, Mode::Agent];
    let sources: [(&str, &[Mode]); 2] = [
        (include_str!("../../../../scenarios/probe/mixed.json"), &all),
        (
            include_str!("../../../../scenarios/probe/mixed-tools.json"),
            &[Mode::Agent],
        ),
    ];
    for (source, supported) in sources {
        for mode in all {
            let result = Scenario::parse(
                mode,
                source,
                Path::new("scenarios/probe/example.json"),
                |path| {
                    assert!(supported.contains(&mode), "不适用的模式应在读取文件前拒绝");
                    let bytes: &[u8] = match path.file_name().unwrap().to_str().unwrap() {
                        "image-a.png" => {
                            include_bytes!("../../../../scenarios/probe/fixtures/image-a.png")
                        }
                        "image-b.png" => {
                            include_bytes!("../../../../scenarios/probe/fixtures/image-b.png")
                        }
                        "report-a.pdf" => {
                            include_bytes!("../../../../scenarios/probe/fixtures/report-a.pdf")
                        }
                        "report-b.pdf" => {
                            include_bytes!("../../../../scenarios/probe/fixtures/report-b.pdf")
                        }
                        "attachment-summary.json" => {
                            include_bytes!(
                                "../../../../scenarios/probe/schemas/attachment-summary.json"
                            )
                        }
                        _ => panic!("内置场景引用了未知附件"),
                    };
                    Ok(bytes.to_vec())
                },
            );
            assert_eq!(
                result.is_ok(),
                supported.contains(&mode),
                "{mode:?}: {:?}",
                result.err()
            );
        }
    }
}
