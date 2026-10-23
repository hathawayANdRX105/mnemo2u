//! 设定卡测试：加载、关键词选择、注入渲染、id 唯一性。

use novel_review::style::{duplicate_free, render_cards, select_cards, SettingCardSet};

fn cards() -> SettingCardSet {
    SettingCardSet::from_yaml(
        r#"
cards:
  - id: lin-yue
    kind: character
    keywords: [林越, 越哥哥]
    content: 林越：前朝余孽，凡尔登法典编纂者，惯用左手短刀。
  - id: capital
    kind: world
    keywords: [京城]
    content: 京城：皇城与外城以旧闸分隔，宵禁由侍卫执行。
  - id: no-keywords
    kind: rule
    content: 规则：不可直接引用设定文本。
"#,
    )
    .expect("yaml 应可解析")
}

#[test]
fn select_by_keyword() {
    let set = cards();
    let hit = select_cards(&set, "三日后，京城。林越负手立于城楼之上。");
    let ids: Vec<&str> = hit.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"lin-yue"));
    assert!(ids.contains(&"capital"));
    assert!(!ids.contains(&"no-keywords"), "无关键词卡片不应被选中");
}

#[test]
fn select_empty_scene_text() {
    let set = cards();
    assert!(select_cards(&set, "").is_empty());
}

#[test]
fn select_no_match() {
    let set = cards();
    assert!(select_cards(&set, "完全无关的文本").is_empty());
}

#[test]
fn render_cards_injection() {
    let set = cards();
    let hit = select_cards(&set, "林越回到京城");
    let rendered = render_cards(&hit);
    assert!(rendered.contains("# 相关设定"));
    assert!(rendered.contains("## lin-yue"));
    assert!(rendered.contains("## capital"));
    assert!(rendered.contains("前朝余孽"));
}

#[test]
fn render_cards_empty_is_empty_string() {
    assert_eq!(render_cards(&[]), "");
}

#[test]
fn duplicate_ids_detected() {
    let set = SettingCardSet::from_yaml(
        r#"
cards:
  - id: a
    kind: character
    keywords: [x]
    content: A
  - id: a
    kind: world
    content: B
"#,
    )
    .expect("yaml 应可解析");
    assert_eq!(set.duplicate_ids(), vec!["a".to_string()]);
    assert!(!duplicate_free(&set));
}

#[test]
fn clean_set_has_no_duplicates() {
    assert!(duplicate_free(&cards()));
}

#[test]
fn keyword_card_without_keywords_never_matches() {
    let set = cards();
    // 场景包含其 content 文本也不应命中（只认 keywords）。
    assert!(select_cards(&set, "不可直接引用设定文本").is_empty());
}
