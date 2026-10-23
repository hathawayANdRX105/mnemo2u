//! 规则配置加载回归测试：规则节结构与加载语义的行为断言。

use std::fs;

use novel_review::config::{default_rules_path, load_rules, RegexRule, ReviewRules};

fn rules() -> ReviewRules {
    load_rules(&default_rules_path()).expect("默认规则文件应可加载")
}

fn names(items: &[RegexRule]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

#[test]
fn draft_regex_tokens_include_bushi() {
    let cfg = rules();
    let token_names = names(&cfg.draft.regex_rules.tokens);
    assert!(token_names.contains(&"不是"));
}

#[test]
fn draft_template_rules_include_bushi_ershi() {
    let cfg = rules();
    let names: Vec<&str> = cfg
        .draft
        .template_rules
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert!(names.contains(&"不是A而是B"));
}

#[test]
fn draft_tracked_terms_include_leide() {
    let cfg = rules();
    let terms: Vec<&str> = cfg
        .draft
        .tracked_terms
        .iter()
        .map(|item| item.term.as_str())
        .collect();
    assert!(terms.contains(&"雷德"));
}

#[test]
fn draft_ending_label_display_imagery() {
    let cfg = rules();
    assert_eq!(
        cfg.draft
            .ending_labels
            .display
            .get("imagery_coda")
            .map(String::as_str),
        Some("意象压轴")
    );
}

#[test]
fn plan_required_headings_chapter_function() {
    let cfg = rules();
    let chapter_groups = cfg
        .plan
        .required_headings
        .get("chapter-plan")
        .expect("chapter-plan 分组应存在");
    let flat: Vec<&str> = chapter_groups
        .iter()
        .flatten()
        .map(String::as_str)
        .collect();
    assert!(flat.contains(&"本章功能"));
}

#[test]
fn plan_function_rules_chapter_conflict() {
    let cfg = rules();
    let conflict = cfg
        .plan
        .function_rules
        .chapter
        .get("conflict")
        .expect("chapter.conflict 词表应存在");
    assert!(conflict.iter().any(|term| term == "冲突"));
}

#[test]
fn inactive_candidates_are_not_active_templates() {
    let cfg = rules();
    let inactive: std::collections::HashSet<&str> = cfg
        .draft
        .inactive_template_candidates
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    let active: std::collections::HashSet<&str> = cfg
        .draft
        .template_rules
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert!(inactive.is_disjoint(&active));
}

#[test]
fn missing_plan_section_errors() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("rules.yaml");
    fs::write(&path, "draft:\n  regex_rules:\n").expect("写入临时规则");
    assert!(load_rules(&path).is_err(), "缺少 plan 节时应报错");
}

#[test]
fn invalid_yaml_errors() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("rules.yaml");
    fs::write(&path, "draft: [unclosed").expect("写入临时规则");
    assert!(load_rules(&path).is_err(), "非法 YAML 时应报错");
}
