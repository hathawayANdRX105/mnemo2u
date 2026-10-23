//! 规则语域（register）契约测试。
//!
//! 来自真语料矩阵校验（/tmp/corpus 五类语料）：文学与轻小说的正常用法在
//! colloquial 规则上大量误报（鲁迅「不」15 次、「却」7 次，AI 集 0）。
//! 修复不是关规则，而是给规则标语域、报告携带，让评审按作品实际语域判读。
//!
//! 锁死：
//! - `RegexRule.register` 缺省 neutral；显式值透传到 `CompiledRule`/`RegexMetric`；
//! - `HardFlag.register` 对规则来源透传、非规则来源为 neutral；
//! - review.yaml 已标注的规则带正确语域，且取值在文档化词表内；
//! - 真语料上鲁迅命中 common 规则的标签可见（防回归）。

use std::path::{Path, PathBuf};

use novel_review::audit::draft::{self, ReportFormat, RunOptions};
use novel_review::config::{default_rules_path, load_rules};
use novel_review::rules::build_rule_metrics;

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn compile(rules_yaml: &str) -> Vec<novel_review::rules::CompiledRule> {
    // 直接构造 RegexRule 走 compile：register 缺省与显式两条路径。
    let rule: novel_review::config::RegexRule = serde_yaml::from_str(&format!(
        "name: 测试\npattern: 不\nmax_per_10k: 100.0\n{rules_yaml}"
    ))
    .expect("RegexRule 应可解析");
    vec![novel_review::rules::CompiledRule::compile(&rule).expect("应可编译")]
}

#[test]
fn register_defaults_to_neutral() {
    let rules = compile("note: 无语域标注");
    assert_eq!(rules[0].register, "neutral", "缺省 register 必须为 neutral");
}

#[test]
fn register_flows_through_metrics() {
    let rules = compile("note: 文学语域\nregister: literary");
    let lines = vec!["他没有错。".to_string()];
    let (metrics, _) = build_rule_metrics(&rules, &lines, 5, false, 10);
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0].register, "literary", "语域必须透传到 metric");
}

#[test]
fn review_yaml_marks_literary_false_positives() {
    let rules = load_rules(&default_rules_path()).expect("规则文件应可加载");
    let token = rules
        .draft
        .regex_rules
        .tokens
        .iter()
        .find(|t| t.name == "不")
        .expect("应有 token「不」");
    assert_eq!(
        token.register.as_deref(),
        Some("common"),
        "「不」在所有人写语域高频（散文211/网文161/古典160/文学160/轻小说133 per 10k）而 AI 集 0，须标 common"
    );
    let que = rules
        .draft
        .regex_rules
        .tokens
        .iter()
        .find(|t| t.name == "却")
        .expect("应有 token「却」");
    assert_eq!(que.register.as_deref(), Some("literary"));
}

#[test]
fn review_yaml_marks_colloquial_fillers() {
    let rules = load_rules(&default_rules_path()).expect("规则文件应可加载");
    let ran = rules
        .draft
        .regex_rules
        .tokens
        .iter()
        .find(|t| t.name == "然后")
        .expect("应有 token「然后」");
    assert_eq!(
        ran.register.as_deref(),
        Some("literary,lightnovel,webnovel"),
        "「然后」在文学(33.8)/轻小说(12.4)/网文(12.8)高频而 AI 集 0，非纯口语"
    );
}

#[test]
fn unmarked_rules_stay_neutral_in_yaml() {
    let rules = load_rules(&default_rules_path()).expect("规则文件应可加载");
    let unmarked = rules
        .draft
        .regex_rules
        .tokens
        .iter()
        .filter(|t| t.register.is_none())
        .count();
    assert!(
        unmarked > 0,
        "大部分 token 无语域标注（走 neutral 默认），不应全量标"
    );
}

/// 词表白名单：所有 register 标注必须落在文档化集合内，逗号组合合法。
/// 防手滑写成 `Lit`/`网文` 之类词表外取值（报告原样透出，错值会直接误导评审）。
#[test]
fn register_values_stay_within_vocabulary() {
    const VOCAB: &[&str] = &[
        "colloquial",
        "literary",
        "classical",
        "lightnovel",
        "webnovel",
        "common",
        "neutral",
    ];
    let rules = load_rules(&default_rules_path()).expect("规则文件应可加载");
    let sections: Vec<(&str, &[novel_review::config::RegexRule])> = vec![
        ("tokens", &rules.draft.regex_rules.tokens),
        ("patterns", &rules.draft.regex_rules.patterns),
        ("phrases", &rules.draft.regex_rules.phrases),
        ("modifiers", &rules.draft.regex_rules.modifiers),
        ("punctuation", &rules.draft.regex_rules.punctuation),
        (
            "punctuation_combos",
            &rules.draft.regex_rules.punctuation_combos,
        ),
    ];
    let mut marked = 0usize;
    for (section, list) in sections {
        for rule in list {
            let Some(reg) = rule.register.as_deref() else {
                continue;
            };
            marked += 1;
            let parts: Vec<&str> = reg.split(',').map(str::trim).collect();
            assert!(!parts.is_empty(), "{section}:{:?} register 为空", rule.name);
            for p in parts {
                assert!(
                    VOCAB.contains(&p),
                    "{section}:{:?} 的词表外 register {p:?}（整值 {reg:?}）",
                    rule.name
                );
            }
        }
    }
    assert!(
        marked >= 20,
        "矩阵结论应已落到多条规则上，当前仅 {marked} 条带 register"
    );
}

/// 端到端：真语料（若存在）上鲁迅的「不」命中带 common 标签。
/// 语料在 CI 上不存在时跳过（本地/开发者机上验证）。
#[test]
fn lu_xun_hits_literary_register_end_to_end() {
    let corpus = fixture("../.wt-corpus/literature/lx-guxiang.md");
    let alt = Path::new("/tmp/corpus/literature/lx-guxiang.md").to_path_buf();
    let path = if corpus.exists() { corpus } else { alt };
    if !path.exists() {
        eprintln!("跳过：鲁迅语料不在 {path:?}");
        return;
    }
    let tmp = tempfile::tempdir().expect("临时目录");
    let out = tmp.path().join("r.json");
    let opts = RunOptions {
        positional: Vec::new(),
        inputs: vec![path],
        sample_limit: 3,
        fail_on_warn: false,
        format: ReportFormat::Json,
        output: Some(out.clone()),
        learn_from: None,
        no_corpus_learning: true,
        rules: None,
    };
    draft::run(&opts).expect("run 应成功");
    let reports: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("读报告")).expect("json");
    let flags = reports[0]["hard_flags"].as_array().expect("数组");
    let bu = flags
        .iter()
        .find(|f| f["name"] == "不")
        .expect("鲁迅文本应命中「不」");
    assert_eq!(
        bu["register"].as_str(),
        Some("common"),
        "端到端必须把 common 标签透传到 hard_flags"
    );
}
