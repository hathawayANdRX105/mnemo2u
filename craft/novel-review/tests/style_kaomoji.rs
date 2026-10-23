//! 轻小说风格测试：颜文字映射、确定性挑选、密度统计与频率上限。

use novel_review::style::{
    count_kaomoji, kaomoji_density_per_10k, kaomoji_over_cap, select_kaomoji, StyleProfileSet,
};

fn light_novel() -> novel_review::style::StyleProfile {
    let set = StyleProfileSet::from_yaml(
        r#"
styles:
  - id: light-novel
    name: 轻小说
    description: 口语化、内心独白多、颜文字点缀吐槽。
    kaomoji:
      无语:
        - "(°Д°)"
        - "(￣▽￣\")"
      开心:
        - "(≧▽≦)"
    kaomoji_cap_per_10k: 30
"#,
    )
    .expect("yaml 应可解析");
    set.find("light-novel").expect("应找到").clone()
}

#[test]
fn kaomoji_map_loads() {
    let p = light_novel();
    assert_eq!(p.kaomoji.len(), 2);
    assert_eq!(p.kaomoji["无语"].len(), 2);
    assert_eq!(p.kaomoji_cap_per_10k, 30);
}

#[test]
fn select_is_deterministic_for_same_context() {
    let p = light_novel();
    let a = select_kaomoji(&p, "无语", "这展开太突然了");
    let b = select_kaomoji(&p, "无语", "这展开太突然了");
    assert!(a.is_some());
    assert_eq!(a, b, "同上下文必须同结果");
    assert!(p.kaomoji["无语"].contains(&a.unwrap().to_string()));
}

#[test]
fn select_differs_across_contexts_sometimes() {
    // 不保证必不同，但候选集内必须合法。
    let p = light_novel();
    for ctx in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        let picked = select_kaomoji(&p, "无语", ctx).expect("应选出");
        assert!(p.kaomoji["无语"].contains(&picked.to_string()));
    }
}

#[test]
fn select_unknown_emotion_returns_none() {
    let p = light_novel();
    assert!(select_kaomoji(&p, "悲伤", "任何上下文").is_none());
}

#[test]
fn count_and_density() {
    let p = light_novel();
    let text = "(°Д°) 你说什么？(≧▽≦) 太好了。(°Д°) 又来了。";
    assert_eq!(count_kaomoji(&p, text), 3);
    let density = kaomoji_density_per_10k(&p, text);
    assert!(density > 0.0);
    // 短文本塞 3 个颜文字，密度必然超 30/万字。
    assert!(kaomoji_over_cap(&p, text), "短文高密度应判超限: {density}");
}

#[test]
fn over_cap_detected_on_long_text() {
    let p = light_novel();
    // 一万字规模：塞 60 个颜文字，超过 30/万字上限。
    let mut text = String::new();
    while text.chars().count() < 10000 {
        text.push_str("他愣住了。");
    }
    for _ in 0..60 {
        text.push_str("(°Д°)");
    }
    let density = kaomoji_density_per_10k(&p, &text);
    assert!(density > 30.0, "密度应超上限，实际 {density}");
    assert!(kaomoji_over_cap(&p, &text));
}

#[test]
fn empty_text_zero_density() {
    let p = light_novel();
    assert_eq!(kaomoji_density_per_10k(&p, "   \n  "), 0.0);
    assert!(!kaomoji_over_cap(&p, ""));
}

#[test]
fn yaml_config_file_renders_kaomoji_section() {
    // 仓库自带样例配置（style-inspect 的输出源）。
    let path = std::path::Path::new("configs/styles/light-novel.yaml");
    if !path.exists() {
        eprintln!("跳过：{path:?} 不在测试工作目录");
        return;
    }
    let set = StyleProfileSet::load(path).expect("样例配置应可加载");
    let prompt = set.render_prompt("light-novel").expect("应渲染");
    assert!(prompt.contains("## 颜文字映射"));
    assert!(prompt.contains("(°Д°)"));
    assert!(prompt.contains("万字"));
}
