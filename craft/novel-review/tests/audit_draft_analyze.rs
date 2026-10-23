//! 规则解析与 ngram 层行为断言：
//! - `test_analyze_text_hits_yaml_template_rule`：`analyze_text` 携带 YAML
//!   模板库/词库（默认 `configs/rules/review.yaml`）时，输入文本必须在内置
//!   `patterns` 节（label `不是A而是B`，即 YAML `template_rules` 中与内置规则
//!   重名被过滤的那条模板对应的内置对照句规则）上产生命中；
//! - `test_ngram_terms_keep_maximal_repetition`：`collect_ngram_terms` 去重
//!   保留最大重复短语本身（同计数的子串不覆盖母串）。

use novel_review::audit::draft::{analyze_text, collect_ngram_terms, DraftContext};
use novel_review::config::{default_rules_path, load_rules};
use novel_review::rules::build_template_bank;

#[test]
fn analyze_text_hits_negation_contrast_pattern() {
    let rules = load_rules(&default_rules_path()).expect("默认规则文件应可加载");
    let ctx = DraftContext::new(rules.clone()).expect("DraftContext 应可装配");
    let template_bank = build_template_bank(ctx.draft_rules());
    let term_bank = ctx.draft_rules().tracked_terms.clone();

    let text = "这不是冲动，而是判断。\n那不是巧合，而是设计。\n他不是退让，而是换位。\n";
    let analysis = analyze_text(&ctx, text, "fixture", &template_bank, &term_bank, None, 3)
        .expect("analyze_text 不应报错");

    let hits = analysis
        .patterns
        .iter()
        .filter(|m| m.name == "不是A而是B")
        .collect::<Vec<_>>();
    assert!(!hits.is_empty(), "expected YAML pattern 不是A而是B to hit");
    assert!(hits[0].count >= 1, "命中数应 >= 1，实际 {}", hits[0].count);
}
/// ngram 去重钉住用例：
/// `"继续调查" * 4` 下 2/3/4-gram 均过 `min_count_by_size={2:2,3:2,4:2}`，
/// 去重后最大重复短语 `("继续调查", 4)` 必须保留（其 2/3 字子串被覆盖，
/// 但母串本身不会被同计数的子串反向覆盖）。
#[test]
fn ngram_terms_keep_maximal_repetition() {
    let rules = load_rules(&default_rules_path()).expect("默认规则文件应可加载");
    let ctx = DraftContext::new(rules.clone()).expect("DraftContext 应可装配");
    let terms = collect_ngram_terms(
        &ctx,
        &"继续调查".repeat(4),
        &[(2, 2), (3, 2), (4, 2)],
        false,
    );
    assert!(
        terms.contains(&("继续调查".to_string(), 4)),
        "最大重复短语 (\"继续调查\", 4) 应保留，实际 {:?} 前若干项",
        &terms[..terms.len().min(8)]
    );
}

/// 归因句豁免钉住用例（来自扩展真语料矩阵，见 configs/rules/REGISTER-MATRIX.md）：
/// 话语归因（「段誉道」「他说」）在人写经典与 AI 章出现占比相近、无判别力，
/// 但曾是人写长篇 clause_prefixes 标记的最大来源（天龙 22 条里 16 条）。
/// 归因形态必须被 `learned_term_window.attribution_suffixes` 滤掉；
/// 人名裸重复（AI 的「亚历克斯」型信号）必须保留。
#[test]
fn clause_prefixes_skip_attribution_leads() {
    let rules = load_rules(&default_rules_path()).expect("默认规则文件应可加载");
    let ctx = DraftContext::new(rules.clone()).expect("DraftContext 应可装配");
    let template_bank = build_template_bank(ctx.draft_rules());
    let term_bank = ctx.draft_rules().tracked_terms.clone();

    let attribution =
        "段誉道：休得无礼。\n段誉道：得罪了。\n段誉道：小心。\n段誉道：来吧。\n段誉道：看招。\n";
    let analysis = analyze_text(
        &ctx,
        attribution,
        "attr",
        &template_bank,
        &term_bank,
        None,
        3,
    )
    .expect("analyze_text 不应报错");
    assert!(
        analysis.clause_prefixes.is_empty(),
        "纯归因句不应进 clause_prefixes，实际 {:?}",
        analysis.clause_prefixes
    );

    let name_lead = "亚历克斯把门关上。\n".repeat(5);
    let analysis = analyze_text(
        &ctx,
        &name_lead,
        "name",
        &template_bank,
        &term_bank,
        None,
        3,
    )
    .expect("analyze_text 不应报错");
    assert!(
        analysis
            .clause_prefixes
            .iter()
            .any(|p| p.phrase == "亚历克斯"),
        "人名裸重复必须保留（AI 重复起手信号），实际 {:?}",
        analysis.clause_prefixes
    );
}
