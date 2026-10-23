//! `audit-draft` 规则扫描与指标计算。
//!
//! 语义契约：
//! - [`find_hits`] 逐行统计非重叠匹配数，前 `sample_limit` 个命中行各记一条样本（整行 strip）；
//! - [`density`] 以去掉换行符后的全文字符数为分母；
//! - `per_10k` 经 [`round2`]（二进制精确值 + 半偶舍入）；
//! - 超标判定为 `per_10k > max_per_10k` 严格大于。
//!
//! 正则引擎用 `fancy_regex`：`review.yaml` 的模板含回引用（`\1`），
//! `regex` crate 不支持，而 `fancy_regex` 为兼容 superset。

use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result};
use fancy_regex::Regex;
use serde::Serialize;

use crate::config::{DraftConfig, RegexRule, TemplateRule, TrackedTerm};

/// 单行命中样本（行号 + 整行 strip）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub line_no: usize,
    pub snippet: String,
}

/// 将 `x` 四舍五入到 2 位小数：对二进制精确值做半偶舍入。
///
/// 与「先算精确值再半偶舍入」逐位一致：实现内用整数还原精确二进制值正确舍入，
/// 避免 `x * 100.0` 先引入一次乘法舍入造成的双重舍入偏差（详见函数内注释）。
#[must_use]
pub fn round2(x: f64) -> f64 {
    debug_assert!(x >= 0.0 && x.is_finite(), "仅用于非负有限的密度值");
    if x == 0.0 {
        return 0.0;
    }
    // 对 x*100 的二进制精确值做正确舍入（半偶）。
    // 直接算 x*100.0 会先引入一次乘法舍入：2.675 的存储值略小于 2.675，
    // 但 2.675f64 * 100.0 舍入到 267.5，半偶得 2.68；期望值 2.67。
    // 这里用 u128 整数还原精确二进制值：x = mantissa * 2^exponent。
    let bits = x.to_bits();
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let scaled = (mantissa as u128) * 100; // x * 100 = scaled * 2^exponent
    let nearest: u128 = if exponent >= 0 {
        scaled << exponent
    } else {
        let shift = (-exponent) as u32;
        if shift >= 128 {
            0
        } else {
            let low = scaled & ((1u128 << shift) - 1);
            let half = 1u128 << (shift - 1);
            let mut quotient = scaled >> shift;
            if low > half || (low == half && quotient & 1 == 1) {
                quotient += 1;
            }
            quotient
        }
    };
    (nearest as f64) / 100.0
}

/// 每万字密度；分母为 0 时返回 0.0。
#[must_use]
pub fn density(count: usize, chars: usize) -> f64 {
    if chars == 0 {
        0.0
    } else {
        count as f64 * 10000.0 / chars as f64
    }
}

/// 逐行计数非重叠匹配；前 `sample_limit` 个命中行各记录一条样本。
#[must_use]
pub fn find_hits(pattern: &Regex, lines: &[String], sample_limit: usize) -> (usize, Vec<Hit>) {
    let mut count = 0_usize;
    let mut hits: Vec<Hit> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_matches = pattern.find_iter(line).filter(|m| m.is_ok()).count();
        if line_matches == 0 {
            continue;
        }
        count += line_matches;
        if hits.len() < sample_limit {
            hits.push(Hit {
                line_no: idx + 1,
                snippet: line.trim().to_string(),
            });
        }
    }
    (count, hits)
}

/// 编译后的正则规则（label 为 patterns 节展示名，其余节为 None）。
pub struct CompiledRule {
    pub name: String,
    pub label: Option<String>,
    pub regex: fancy_regex::Regex,
    pub max_per_10k: f64,
    pub note: String,
    /// 语域标签（`colloquial`/`literary`/`classical`/`lightnovel`/`webnovel`/
    /// `common`，可逗号组合），缺省 neutral。
    pub register: String,
}

impl CompiledRule {
    /// 编译单条规则；pattern 非法时返回带规则名的错误。
    pub fn compile(rule: &RegexRule) -> Result<Self> {
        let regex = fancy_regex::Regex::new(&rule.pattern)
            .with_context(|| format!("规则 {:?} 的正则无法编译: {:?}", rule.name, rule.pattern))?;
        Ok(Self {
            name: rule.name.clone(),
            label: rule.label.clone(),
            regex,
            max_per_10k: rule.max_per_10k,
            note: rule.note.clone().unwrap_or_default(),
            register: rule
                .register
                .clone()
                .unwrap_or_else(|| "neutral".to_string()),
        })
    }

    /// patterns 节用 label 作为展示名（YAML 保证 patterns 全部带 label）。
    fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

/// `build_rule_metrics` 的一条指标。
#[derive(Debug, Clone, Serialize)]
pub struct RegexMetric {
    pub name: String,
    pub count: usize,
    pub per_10k: f64,
    pub max_per_10k: f64,
    pub note: String,
    /// 规则语域（见 `RegexRule::register`；非规则来源的指标为 neutral）。
    pub register: String,
    pub warn: bool,
    pub samples: Vec<Hit>,
}

/// 对一组规则跑指标；`use_label` 为 true 时展示名取 label（patterns 节）。
#[must_use]
pub fn build_rule_metrics(
    rules: &[CompiledRule],
    lines: &[String],
    chars: usize,
    use_label: bool,
    sample_limit: usize,
) -> (Vec<RegexMetric>, bool) {
    let mut metrics = Vec::with_capacity(rules.len());
    let mut warned = false;
    for rule in rules {
        let (count, samples) = find_hits(&rule.regex, lines, sample_limit);
        let per_10k_raw = density(count, chars);
        let flag = per_10k_raw > rule.max_per_10k;
        warned |= flag;
        metrics.push(RegexMetric {
            name: if use_label {
                rule.display_name().to_string()
            } else {
                rule.name.clone()
            },
            count,
            per_10k: round2(per_10k_raw),
            max_per_10k: rule.max_per_10k,
            note: rule.note.clone(),
            register: rule.register.clone(),
            warn: flag,
            samples,
        });
    }
    (metrics, warned)
}

/// `build_tracked_term_metrics` 的单词指标（`category` 在前，字段序固定）。
#[derive(Debug, Clone, Serialize)]
pub struct TrackedMetric {
    pub category: String,
    pub name: String,
    pub count: usize,
    pub per_10k: f64,
    pub max_per_10k: f64,
    pub note: String,
    pub warn: bool,
    pub samples: Vec<Hit>,
}

/// 分类聚合里的活跃词行。
#[derive(Debug, Clone, Serialize)]
pub struct TopTerm {
    pub term: String,
    pub count: usize,
    pub per_10k: f64,
    pub warn: bool,
}

/// `tracked_term_categories` 行：分类聚合、活跃词数与前 8 热门词。
#[derive(Debug, Clone, Serialize)]
pub struct CategoryRow {
    pub category: String,
    pub count: usize,
    pub warn_terms: usize,
    pub active_terms: usize,
    pub warn: bool,
    pub top_terms: Vec<TopTerm>,
}

struct CategoryBucket {
    count: usize,
    warn_terms: usize,
    terms: Vec<TopTerm>,
}

/// 跟踪词密度指标 + 分类聚合。term 按字面量匹配（`re.escape` 语义）。
pub fn build_tracked_term_metrics(
    tracked_terms: &[TrackedTerm],
    lines: &[String],
    chars: usize,
    sample_limit: usize,
) -> Result<(Vec<TrackedMetric>, Vec<CategoryRow>, bool)> {
    let mut metrics = Vec::with_capacity(tracked_terms.len());
    let mut buckets: BTreeMap<&str, CategoryBucket> = BTreeMap::new();
    let mut warned = false;
    for term_rule in tracked_terms {
        let regex = Regex::new(&fancy_regex::escape(&term_rule.term))
            .with_context(|| format!("跟踪词 {:?} 无法编译", term_rule.term))?;
        let (count, samples) = find_hits(&regex, lines, sample_limit);
        let per_10k_raw = density(count, chars);
        let flag = per_10k_raw > term_rule.max_per_10k;
        warned |= flag;
        let per_10k = round2(per_10k_raw);
        metrics.push(TrackedMetric {
            category: term_rule.category.clone(),
            name: term_rule.term.clone(),
            count,
            per_10k,
            max_per_10k: term_rule.max_per_10k,
            note: term_rule.note.clone().unwrap_or_default(),
            warn: flag,
            samples,
        });
        let bucket = buckets
            .entry(term_rule.category.as_str())
            .or_insert_with(|| CategoryBucket {
                count: 0,
                warn_terms: 0,
                terms: Vec::new(),
            });
        bucket.count += count;
        if flag {
            bucket.warn_terms += 1;
        }
        if count > 0 {
            bucket.terms.push(TopTerm {
                term: term_rule.term.clone(),
                count,
                per_10k,
                warn: flag,
            });
        }
    }

    let mut categories = Vec::with_capacity(buckets.len());
    for (category, mut bucket) in buckets {
        let active_terms = bucket.terms.len();
        bucket
            .terms
            .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.term.cmp(&b.term)));
        bucket.terms.truncate(8);
        categories.push(CategoryRow {
            category: category.to_string(),
            count: bucket.count,
            warn_terms: bucket.warn_terms,
            active_terms,
            warn: bucket.warn_terms > 0,
            top_terms: bucket.terms,
        });
    }
    Ok((metrics, categories, warned))
}

/// 模板库：跳过空 pattern、`enabled: false`、以及与内置六组规则重名的条目
/// （重名条目按 `HARDCODED_TEMPLATE_RULE_NAMES` 判定）。
#[must_use]
pub fn build_template_bank(draft: &DraftConfig) -> Vec<TemplateRule> {
    let regex = &draft.regex_rules;
    let hardcoded: HashSet<&str> = regex
        .patterns
        .iter()
        .chain(regex.phrases.iter())
        .chain(regex.tokens.iter())
        .chain(regex.punctuation.iter())
        .chain(regex.punctuation_combos.iter())
        .chain(regex.modifiers.iter())
        .map(|item| item.name.as_str())
        .chain(
            regex
                .patterns
                .iter()
                .filter_map(|item| item.label.as_deref()),
        )
        .collect();
    draft
        .template_rules
        .iter()
        .filter(|rule| {
            !rule.pattern.is_empty()
                && rule.enabled != Some(false)
                && !hardcoded.contains(rule.name.as_str())
        })
        .cloned()
        .collect()
}

/// `custom_templates` 节指标（`category` 字段在末尾，字段序固定）。
#[derive(Debug, Clone, Serialize)]
pub struct CustomTemplateMetric {
    pub name: String,
    pub count: usize,
    pub per_10k: f64,
    pub max_per_10k: f64,
    pub note: String,
    pub warn: bool,
    pub samples: Vec<Hit>,
    pub category: String,
}

/// 对模板库逐条跑指标（pattern 非法时报错，携带模板名）。
pub fn build_custom_template_metrics(
    bank: &[TemplateRule],
    lines: &[String],
    chars: usize,
    sample_limit: usize,
) -> Result<(Vec<CustomTemplateMetric>, bool)> {
    let mut metrics = Vec::with_capacity(bank.len());
    let mut warned = false;
    for rule in bank {
        let regex = Regex::new(&rule.pattern)
            .with_context(|| format!("模板 {:?} 的正则无法编译", rule.name))?;
        let (count, samples) = find_hits(&regex, lines, sample_limit);
        let per_10k_raw = density(count, chars);
        let flag = per_10k_raw > rule.max_per_10k;
        warned |= flag;
        metrics.push(CustomTemplateMetric {
            name: rule.name.clone(),
            count,
            per_10k: round2(per_10k_raw),
            max_per_10k: rule.max_per_10k,
            note: rule.note.clone(),
            warn: flag,
            samples,
            category: rule.category.clone(),
        });
    }
    Ok((metrics, warned))
}

/// `audit-draft` 规则指标报告（阶段 1a 覆盖的 analysis 顶层节，键名遵循 JSON 契约）。
#[derive(Debug, Serialize)]
pub struct DraftRuleReport<'a> {
    pub source: &'a str,
    pub tokens: Vec<RegexMetric>,
    pub patterns: Vec<RegexMetric>,
    pub phrases: Vec<RegexMetric>,
    pub modifiers: Vec<RegexMetric>,
    pub punctuation: Vec<RegexMetric>,
    pub punctuation_combos: Vec<RegexMetric>,
    pub custom_templates: Vec<CustomTemplateMetric>,
    pub tracked_terms: Vec<TrackedMetric>,
    pub tracked_term_categories: Vec<CategoryRow>,
}

/// 对单篇草稿文本跑全部规则节指标。
///
/// `chars`：去掉换行符后的全文
/// Unicode 码点数（含空白与标点，非正文净字数）。
pub fn audit_draft_report<'a>(
    draft: &DraftConfig,
    text: &str,
    source: &'a str,
    sample_limit: usize,
) -> Result<DraftRuleReport<'a>> {
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let chars = text.chars().filter(|c| *c != '\n').count();
    let compile_all = |rules: &[RegexRule]| -> Result<Vec<CompiledRule>> {
        rules.iter().map(CompiledRule::compile).collect()
    };

    let (tokens, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.tokens)?,
        &lines,
        chars,
        false,
        sample_limit,
    );
    let (patterns, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.patterns)?,
        &lines,
        chars,
        true,
        sample_limit,
    );
    let (phrases, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.phrases)?,
        &lines,
        chars,
        false,
        sample_limit,
    );
    let (modifiers, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.modifiers)?,
        &lines,
        chars,
        false,
        sample_limit,
    );
    let (punctuation, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.punctuation)?,
        &lines,
        chars,
        false,
        sample_limit,
    );
    let (punctuation_combos, _) = build_rule_metrics(
        &compile_all(&draft.regex_rules.punctuation_combos)?,
        &lines,
        chars,
        false,
        sample_limit,
    );
    let bank = build_template_bank(draft);
    let (custom_templates, _) = build_custom_template_metrics(&bank, &lines, chars, sample_limit)?;
    let (tracked_terms, tracked_term_categories, _) =
        build_tracked_term_metrics(&draft.tracked_terms, &lines, chars, sample_limit)?;

    Ok(DraftRuleReport {
        source,
        tokens,
        patterns,
        phrases,
        modifiers,
        punctuation,
        punctuation_combos,
        custom_templates,
        tracked_terms,
        tracked_term_categories,
    })
}
