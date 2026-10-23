//! 规则扫描引擎行为测试：`round`/`finditer` 语义（银行家舍入、重叠命中计数）与聚合排序。

use novel_review::rules::{
    build_rule_metrics, build_tracked_term_metrics, density, find_hits, round2, CompiledRule,
};
use std::sync::LazyLock;

/// `round(x, 2)` 是二进制精确值上的半偶舍入，非 half-up。
#[test]
fn round2_uses_bankers_rounding() {
    // 精确 .5 边界：向偶数舍入
    assert_eq!(round2(0.625), 0.62);
    assert_eq!(round2(0.875), 0.88);
    assert_eq!(round2(2.125), 2.12);
    assert_eq!(round2(2.375), 2.38);
    // IEEE754 存储值决定的经典值：round2(0.635) == 0.64
    assert_eq!(round2(0.635), 0.64);
    assert_eq!(round2(2.675), 2.67);
    // 非边界常规舍入
    assert_eq!(round2(1.234), 1.23);
    assert_eq!(round2(1.236), 1.24);
    assert_eq!(round2(0.0), 0.0);
}

/// count=1、chars=16000 时 per_10k = 0.625，`round2` 得 0.62；
/// 若用 half-up 会得到 0.63 —— 该用例锁死该边界。
#[test]
fn density_rounding_at_boundary() {
    let raw = density(1, 16_000);
    assert_eq!(raw, 0.625);
    assert_eq!(round2(raw), 0.62);
    assert_eq!(round2(density(3, 16_000)), 1.88);
    assert_eq!(density(5, 0), 0.0);
}

static AB_TEST_RE: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| fancy_regex::Regex::new("甲").unwrap());

#[test]
fn find_hits_counts_all_matches_but_one_sample_per_line() {
    let lines: Vec<String> = ["甲甲甲", "无", "甲", "甲甲", "甲"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (count, samples) = find_hits(&AB_TEST_RE, &lines, 2);
    assert_eq!(count, 7, "所有行的非重叠匹配总数");
    assert_eq!(samples.len(), 2, "样本行数封顶 sample_limit");
    assert_eq!(samples[0].line_no, 1);
    assert_eq!(samples[1].line_no, 3);
}

#[test]
fn find_hits_trims_sample_snippets() {
    let lines = vec!["  甲  ".to_string()];
    let (count, samples) = find_hits(&AB_TEST_RE, &lines, 3);
    assert_eq!(count, 1);
    assert_eq!(samples[0].snippet, "甲");
}

static RULES: LazyLock<novel_review::config::ReviewRules> = LazyLock::new(|| {
    novel_review::config::load_rules(&novel_review::config::default_rules_path())
        .expect("默认规则可加载")
});

/// review.yaml 全部正则必须可编译，其中含回引用模式（regex crate 不支持）。
#[test]
fn all_yaml_patterns_compile() {
    let draft = &RULES.draft;
    for group in [
        &draft.regex_rules.tokens,
        &draft.regex_rules.patterns,
        &draft.regex_rules.phrases,
        &draft.regex_rules.modifiers,
        &draft.regex_rules.punctuation,
        &draft.regex_rules.punctuation_combos,
    ] {
        for rule in group {
            CompiledRule::compile(rule).unwrap_or_else(|e| panic!("{:?} 编译失败: {e}", rule.name));
        }
    }
    let bank = novel_review::rules::build_template_bank(draft);
    assert!(!bank.is_empty(), "模板库不应为空");
    for t in &bank {
        fancy_regex::Regex::new(&t.pattern)
            .unwrap_or_else(|e| panic!("模板 {:?} 编译失败: {e}", t.name));
    }
}

/// 回引用模式（AA/AABB/ABAB 重叠词）必须真的能匹配计数。
#[test]
fn backreference_template_counts_repeated_words() {
    let bank = novel_review::rules::build_template_bank(&RULES.draft);
    let overlapped = bank
        .iter()
        .find(|t| t.name == "重叠词节奏")
        .expect("重叠词节奏模板应存在");
    let re = fancy_regex::Regex::new(&overlapped.pattern).expect("可编译");
    // 分支为 一AA / (XY)\4 即 ABAB 式；AA 单词本身不命中
    // （重叠命中实测：缓缓缓缓=1、一笑笑=1、微微=0）
    let lines = vec!["缓缓缓缓地走，一笑笑。".to_string()];
    let (count, _) = find_hits(&re, &lines, 3);
    assert_eq!(count, 2, "缓缓缓缓 与 一笑笑 各命中一次");
}

/// patterns 节展示名取 label，其余节取 name。
#[test]
fn rule_metrics_display_name_follows_label_field() {
    let rules = RULES
        .draft
        .regex_rules
        .patterns
        .iter()
        .map(CompiledRule::compile)
        .collect::<Result<Vec<_>, _>>()
        .expect("patterns 应全部可编译");
    assert!(!rules.is_empty());
    let (metrics, _) = build_rule_metrics(&rules, &["无关文本".to_string()], 100, true, 1);
    for (m, r) in metrics.iter().zip(&rules) {
        assert_eq!(&m.name, r.label.as_ref().expect("patterns 均带 label"));
    }
}

/// 分类聚合：sort by category、词按 (-count, term) 排序、封顶 8。
#[test]
fn tracked_term_categories_aggregate_and_sort() {
    let mk = |term: &str, category: &str| novel_review::config::TrackedTerm {
        category: category.to_string(),
        term: term.to_string(),
        max_per_10k: 100_000.0,
        note: Some("测试".to_string()),
    };
    let terms = vec![
        mk("雷德", "person"),
        mk("林越", "person"),
        mk("京城", "place"),
    ];
    let lines: Vec<String> = ["雷德到了京城，林越也到了。".repeat(4)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let chars = lines[0].chars().count();
    let (metrics, categories, warned) =
        build_tracked_term_metrics(&terms, &lines, chars, 1).expect("指标可计算");
    assert_eq!(metrics.len(), 3);
    assert!(!warned, "阈值 100000 不应触发");
    assert_eq!(
        categories
            .iter()
            .map(|c| c.category.as_str())
            .collect::<Vec<_>>(),
        vec!["person", "place"],
        "分类按字典序"
    );
    let person = &categories[0];
    assert_eq!(person.count, 8, "两词各 4 次");
    assert_eq!(person.active_terms, 2);
    assert_eq!(person.top_terms[0].term, "林越", "同计数按词字典序");
    assert_eq!(person.top_terms[1].term, "雷德");
}

/// 超标判定为严格大于：per_10k == max 不算 warn。
#[test]
fn warn_flag_requires_strictly_greater_density() {
    let mk = |term: &str, max: f64| novel_review::config::TrackedTerm {
        category: "person".to_string(),
        term: term.to_string(),
        max_per_10k: max,
        note: None,
    };
    // 雷德 非重叠命中 2 次、总字数 10000 -> per_10k 恰为 2.0
    let filler = "x".repeat(9996);
    let lines = vec![format!("{filler}雷德雷德")];
    let chars = lines[0].chars().count();
    let (metrics_eq, _, warned_eq) =
        build_tracked_term_metrics(&[mk("雷德", 2.0)], &lines, chars, 0).unwrap();
    assert_eq!(metrics_eq[0].per_10k, 2.0);
    assert!(!metrics_eq[0].warn && !warned_eq, "等于阈值不触发");
    let (_, _, warned_gt) =
        build_tracked_term_metrics(&[mk("雷德", 1.99)], &lines, chars, 0).unwrap();
    assert!(warned_gt, "超过阈值应触发");
}
