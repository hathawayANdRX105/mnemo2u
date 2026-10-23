//! 语料学习拟合回归：目标文件不得计入「自动语料」。
//!
//! 背景（真语料 novel/novel1 校验发现）：默认语料包含被分析章节自身时，
//! 「语料学到的高频词」由目标自己的用词统计出来，再判目标过密——
//! 自我实现、恒响（单章语料下 learned_filters 10 条噪声）。
//! 修复：`build_corpus_profile` 增加 `exclude`，`audit-draft` 自动定位
//! 语料时排除本次分析目标；显式 `--learn-from` 仍尊重用户选择。
//!
//! 本测试锁死该契约：
//! - 语料 = 目标自身 → 无独立语料，learned_filters 全消；
//! - 存在独立兄弟章节 → 语料只含兄弟，本书级信号保留。

use std::path::{Path, PathBuf};

use novel_review::audit::draft::{self, ReportFormat, RunOptions};

fn run_json(inputs: Vec<PathBuf>, output: &Path) -> serde_json::Value {
    let opts = RunOptions {
        positional: Vec::new(),
        inputs,
        sample_limit: 3,
        fail_on_warn: false,
        format: ReportFormat::Json,
        output: Some(output.to_path_buf()),
        learn_from: None,
        no_corpus_learning: false,
        rules: None,
    };
    draft::run(&opts).expect("draft run should succeed");
    let text = std::fs::read_to_string(output).expect("report file must exist");
    serde_json::from_str(&text).expect("valid json")
}

fn learned_flag_names(analysis: &serde_json::Value) -> Vec<String> {
    analysis["hard_flags"]
        .as_array()
        .map(|flags| {
            flags
                .iter()
                .filter(|f| f["section"] == "learned_filters")
                .map(|f| f["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 写一个带重复用词（角色名/口头禅）的章节。
fn write_chapter(dir: &Path, name: &str) -> PathBuf {
    std::fs::create_dir_all(dir).expect("创建目录");
    let path = dir.join(name);
    std::fs::write(
        &path,
        "雷德把登记板往前一递，莉莉安没接。\n\
         雷德又说了一遍，莉莉安还是没接。\n\
         她把脚步放慢，带着他转进接待廊后方的小坡道。\n\
         雷德把手插进外套口袋，隔着布料按了按里头那几样硬东西。\n\
         莉莉安骂得很平，一字一字都带账本味。\n",
    )
    .expect("写章节");
    path
}

#[test]
fn solo_corpus_never_learns_from_its_own_target() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let novel = tmp.path().join("novel");
    let ch01 = write_chapter(&novel.join("drafts"), "ch01.md");

    let out = tmp.path().join("report.json");
    let reports = run_json(vec![ch01.clone()], &out);

    assert_eq!(reports.as_array().map(Vec::len), Some(1));
    let analysis = &reports[0];
    // 语料只有目标自身、排除后为空 → 无独立语料画像（enabled=false）。
    let profile = analysis["corpus_profile"].as_object().expect("语料画像节");
    assert_eq!(
        profile["enabled"].as_bool(),
        Some(false),
        "无独立语料时不得启用语料画像（不得自学）: {profile:?}"
    );
    assert_eq!(profile["source_count"].as_u64(), Some(0));
    // 循环指标必须消失（修复前此处恒有 雷德/莉莉安/得很 等 10 条）。
    assert!(
        learned_flag_names(analysis).is_empty(),
        "目标自身不得作为语料来源: {:?}",
        learned_flag_names(analysis)
    );
}

#[test]
fn independent_sibling_corpus_keeps_book_level_signal() {
    let tmp = tempfile::tempdir().expect("临时目录");
    let novel = tmp.path().join("novel");
    let drafts = novel.join("drafts");
    let ch01 = write_chapter(&drafts, "ch01.md");
    let ch02 = write_chapter(&drafts, "ch02.md");

    let out = tmp.path().join("report.json");
    let reports = run_json(vec![ch01.clone()], &out);

    let analysis = &reports[0];
    let profile = analysis["corpus_profile"]
        .as_object()
        .expect("应有语料画像");
    // 语料只含兄弟章节（目标被排除），不再是「目标+兄弟」。
    assert_eq!(profile["enabled"].as_bool(), Some(true), "有独立语料应启用");
    assert_eq!(
        profile["source_count"].as_u64(),
        Some(1),
        "语料应只含独立兄弟章节"
    );
    assert!(profile["chars"].as_u64().unwrap() > 0);
    // 同时分析两章时两个目标互斥，语料为空（各自都不是对方的独立语料）。
    let out2 = tmp.path().join("report2.json");
    let reports2 = run_json(vec![ch01.clone(), ch02], &out2);
    for a in reports2.as_array().expect("数组") {
        let p = a["corpus_profile"].as_object().expect("语料画像节");
        assert_eq!(
            p["enabled"].as_bool(),
            Some(false),
            "全部目标互为语料时不应构造画像"
        );
        assert_eq!(p["source_count"].as_u64(), Some(0));
        assert!(learned_flag_names(a).is_empty());
    }
}
