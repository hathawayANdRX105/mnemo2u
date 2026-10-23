//! 风格画像测试：加载、prompt 渲染、边界。

use novel_review::style::StyleProfileSet;

fn set() -> StyleProfileSet {
    StyleProfileSet::from_yaml(
        r#"
styles:
  - id: webnovel
    name: 网文爽文
    description: 快节奏、对话驱动、章末留钩子。
    expect:
      - 短句连发
      - 对话驱动
    forbid:
      - 解释腔替读者下结论
    samples:
      - "他冷笑一声：'你也配？'"
  - id: wuxia
    name: 武侠
    description: 含蓄留白，动作具体。
"#,
    )
    .expect("yaml 应可解析")
}

#[test]
fn load_and_find() {
    let s = set();
    let p = s.find("webnovel").expect("应找到 webnovel");
    assert_eq!(p.name, "网文爽文");
    assert_eq!(p.expect.len(), 2);
    assert_eq!(p.forbid.len(), 1);
    assert_eq!(p.samples.len(), 1);
    assert!(s.find("nope").is_none());
}

#[test]
fn render_prompt_includes_all_sections() {
    let s = set();
    let prompt = s.render_prompt("webnovel").expect("应渲染");
    assert!(prompt.contains("# 风格：网文爽文"));
    assert!(prompt.contains("快节奏"));
    assert!(prompt.contains("## 期望特征"));
    assert!(prompt.contains("短句连发"));
    assert!(prompt.contains("## 禁忌特征"));
    assert!(prompt.contains("避免：解释腔"));
    assert!(prompt.contains("## 示例"));
    assert!(prompt.contains("你也配"));
}

#[test]
fn render_prompt_omits_empty_sections() {
    let s = set();
    let prompt = s.render_prompt("wuxia").expect("应渲染");
    assert!(prompt.contains("# 风格：武侠"));
    assert!(prompt.contains("含蓄留白"));
    assert!(!prompt.contains("## 期望特征"), "空 expect 不应出现小节");
    assert!(!prompt.contains("## 禁忌特征"), "空 forbid 不应出现小节");
    assert!(!prompt.contains("## 示例"), "空 samples 不应出现小节");
}

#[test]
fn render_prompt_unknown_id_returns_none() {
    assert!(set().render_prompt("missing").is_none());
}

#[test]
fn invalid_yaml_errors() {
    assert!(StyleProfileSet::from_yaml("styles: [").is_err());
}
