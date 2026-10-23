//! `paths` 节：目录布局可配。
//!
//! 覆盖设计书的三条验收：改目录名后类型判定跟着变、镜像路径跟着变、
//! 索引范围跟着变；不写 `paths` 段时行为与默认值一致。

use std::fs;
use std::path::{Path, PathBuf};

use novel_review::config::{load_rules, PathsConfig};
use novel_review::stats::plan::stats_path_for;

fn rules_with_paths(paths_yaml: &str) -> PathsConfig {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("review.yaml");
    let base =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/rules/review.yaml"))
            .expect("读基准规则");
    // 基准规则自带 paths 段；测试要的是「换掉它」，所以先剥掉再拼。
    // paths 段是文件第一段，draft 段紧随其后——按顶层键切。
    let base = match base.find("\ndraft:") {
        Some(idx) => base[idx..].to_string(),
        None => base,
    };
    fs::write(&path, format!("{base}\n{paths_yaml}\n")).expect("写规则");
    load_rules(&path).expect("解析规则").paths
}

fn paths_of(yaml: &str) -> PathsConfig {
    rules_with_paths(yaml)
}

#[test]
fn default_layout_matches_the_built_in_names() {
    let paths = paths_of("paths:\n  cards: concept/cards\n  drafts: drafts\n  arc_plan: arc-plan\n  story_plan: story-plan\n  chapter_plan: chapter-plan\n");
    assert_eq!(paths.cards, "concept/cards");
    assert_eq!(paths.drafts, "drafts");
    assert_eq!(paths.arc_plan, "arc-plan");
    assert_eq!(paths.story_plan, "story-plan");
    assert_eq!(paths.chapter_plan, "chapter-plan");
}

#[test]
fn renamed_plan_dir_changes_the_mirror_path() {
    let paths = paths_of("paths:\n  cards: concept/cards\n  drafts: drafts\n  arc_plan: arc-plan\n  story_plan: story-plan\n  chapter_plan: chapters\n");
    let plan = Path::new("/novel/chapters/story-1/ch01.md");
    let mirrored = stats_path_for(plan, None, &paths).expect("算镜像路径");
    assert_eq!(
        mirrored,
        PathBuf::from("/novel/chapters-stats/story-1/ch01.md"),
        "镜像目录跟着配置改，不跟着内置常量改"
    );
}

#[test]
fn renamed_plan_dir_is_still_recognised_as_a_plan_dir() {
    let paths = paths_of("paths:\n  cards: concept/cards\n  drafts: drafts\n  arc_plan: arc-plan\n  story_plan: story-plan\n  chapter_plan: chapters\n");
    assert_eq!(
        paths.plan_type_of(Path::new("/novel/chapters/story-1/ch01.md")),
        Some("chapter-plan"),
        "配置的目录名仍能认出层类型"
    );
    assert_eq!(
        paths.plan_type_of(Path::new("/novel/unknown/ch01.md")),
        None,
        "不在任何配置目录下的路径不认成 plan"
    );
}

#[test]
fn doc_globs_follow_the_configured_names() {
    let paths = paths_of("paths:\n  cards: lore\n  drafts: text\n  arc_plan: book/arc\n  story_plan: book/story\n  chapter_plan: book/chapter\n");
    let globs = paths.doc_globs();
    assert!(globs.contains(&"lore/**/*.md".to_string()), "{globs:?}");
    assert!(globs.contains(&"text/**/*.md".to_string()), "{globs:?}");
    assert!(
        globs.contains(&"book/chapter/**/*.md".to_string()),
        "{globs:?}"
    );
    assert!(
        !globs.contains(&"drafts/**/*.md".to_string()),
        "旧名字不该再出现：{globs:?}"
    );
}

#[test]
fn nested_story_dirs_do_not_confuse_the_drafts_lookup() {
    // 布局是 drafts/arc1/story1/ch01.md：正文目录是 drafts，不是 drafts/arc1。
    let paths = paths_of("paths:\n  cards: concept/cards\n  drafts: drafts\n  arc_plan: arc-plan\n  story_plan: story-plan\n  chapter_plan: chapter-plan\n");
    let arc_layer = Path::new("/novel/drafts/arc1");
    assert_eq!(
        paths.drafts_root_component(arc_layer),
        None,
        "arc 层不是正文目录"
    );
    let drafts = Path::new("/novel/drafts");
    assert_eq!(
        paths.drafts_root_component(drafts),
        Some(2),
        "正文目录本身要能认出来（分量：/ novel drafts）"
    );
    assert_eq!(
        paths.drafts_dir_component(Path::new("/novel/drafts/arc1/story1/ch01.md")),
        Some(2),
        "文件路径落在正文目录下"
    );
}

#[test]
fn multi_level_cards_dir_matches_as_a_sequence() {
    let paths = paths_of("paths:\n  cards: lore/cards\n  drafts: text\n  arc_plan: book/arc\n  story_plan: book/story\n  chapter_plan: book/chapter\n");
    assert_eq!(paths.cards_glob(), "lore/cards/**/*.md");
    assert_eq!(
        paths.cards_dir_component(Path::new("/novel/lore/cards/characters/a.md")),
        Some(2),
        "分量：/ novel lore cards —— cards 段起点"
    );
    assert_eq!(
        paths.cards_dir_component(Path::new("/novel/other/cards/a.md")),
        None,
        "只匹配连续的分量序列，不匹配同名末段"
    );
}
