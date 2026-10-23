//! 模板库/词库/内置规则名查表，以及 CATALOG.json 与 SUMMARY.md 构建。

use super::*;

use super::aggregate::{
    aggregate_candidate_families, aggregate_candidates, aggregate_deposition_targets,
    build_learning_anchors, build_writeback_queue,
};

/// `load_bank_names`：`name` → 模板库名集合；`term` → 词库名集合。
fn load_bank_names(draft: &crate::config::DraftConfig, key: &str) -> HashSet<String> {
    if key == "name" {
        draft
            .template_rules
            .iter()
            .filter(|rule| !rule.name.is_empty())
            .map(|rule| rule.name.clone())
            .collect()
    } else {
        draft
            .tracked_terms
            .iter()
            .filter(|term| !term.term.is_empty())
            .map(|term| term.term.clone())
            .collect()
    }
}

/// `load_builtin_rule_template_names`（六组规则的 label+name 并集）。
fn load_builtin_rule_template_names(draft: &crate::config::DraftConfig) -> HashSet<String> {
    let groups: Vec<Vec<&RegexRule>> = vec![
        draft.regex_rules.patterns.iter().collect(),
        draft.regex_rules.phrases.iter().collect(),
        draft.regex_rules.tokens.iter().collect(),
        draft.regex_rules.punctuation.iter().collect(),
        draft.regex_rules.punctuation_combos.iter().collect(),
        draft.regex_rules.modifiers.iter().collect(),
    ];
    let mut names: HashSet<String> = HashSet::new();
    for group in &groups {
        for rule in group {
            if let Some(label) = &rule.label {
                if !label.is_empty() {
                    names.insert(label.clone());
                }
            }
            if !rule.name.is_empty() {
                names.insert(rule.name.clone());
            }
        }
    }
    names
}

/// `load_builtin_rule_term_names`（三组规则的 name 并集）。
fn load_builtin_rule_term_names(draft: &crate::config::DraftConfig) -> HashSet<String> {
    let groups: Vec<Vec<&RegexRule>> = vec![
        draft.regex_rules.tokens.iter().collect(),
        draft.regex_rules.phrases.iter().collect(),
        draft.regex_rules.modifiers.iter().collect(),
    ];
    let mut names: HashSet<String> = HashSet::new();
    for group in &groups {
        for rule in group {
            if !rule.name.is_empty() {
                names.insert(rule.name.clone());
            }
        }
    }
    names
}

/// `build_catalog_markdown` 的分节聚合输入（避免 9 参函数触发 clippy）。
#[derive(Debug, Clone, Copy)]
pub struct CatalogSections<'a> {
    pub template_candidates: &'a [Value],
    pub template_families: &'a [Value],
    pub term_candidates: &'a [Value],
    pub keep_candidates: &'a [Value],
    pub deposition_targets: &'a [Value],
    pub learning_anchors: &'a [Value],
    pub writeback_queue: &'a [Value],
}

pub fn build_catalog_markdown(
    novel_dir: &Path,
    stories: usize,
    sections: &CatalogSections,
) -> String {
    let CatalogSections {
        template_candidates,
        template_families,
        term_candidates,
        keep_candidates,
        deposition_targets,
        learning_anchors,
        writeback_queue,
    } = *sections;
    let get = |item: &Value, key: &str| -> String {
        item.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let gi = |item: &Value, key: &str| item.get(key).and_then(Value::as_i64).unwrap_or(0);

    let mut lines: Vec<String> = vec!["# Template Candidate Catalog".to_string(), String::new()];
    lines.push(format!(
        "- novel: `{}`",
        novel_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    lines.push(format!("- stories: `{stories}`"));
    lines.push(format!(
        "- source: `{}`",
        novel_dir.join("draft-stats").display()
    ));
    lines.push(String::new());

    lines.push("## Learning Anchors".to_string());
    if !learning_anchors.is_empty() {
        for item in learning_anchors {
            lines.push(format!(
                "- `{kind}` `{bucket}::{name}` stories=`{stories}` total=`{total}` -> `{target}`",
                kind = get(item, "kind"),
                bucket = get(item, "bucket"),
                name = get(item, "name"),
                stories = gi(item, "story_count"),
                total = gi(item, "count"),
                target = get(item, "suggested_target"),
            ));
            lines.push(format!("  说明：{}", get(item, "reason")));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Merged Families".to_string());
    if !template_families.is_empty() {
        for item in template_families.iter().take(16) {
            let buckets = item
                .get("buckets")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|b| b.as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            lines.push(format!(
                "- `{name}` stories=`{stories}` total=`{total}` buckets=`{buckets}`",
                name = get(item, "name"),
                stories = gi(item, "story_count"),
                total = gi(item, "count"),
            ));
            let sample = get(item, "sample");
            if !sample.is_empty() {
                lines.push(format!("  样例：{sample}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Template Candidates".to_string());
    if !template_candidates.is_empty() {
        for item in template_candidates.iter().take(20) {
            lines.push(format!(
                "- `{bucket}::{name}` stories=`{stories}` total=`{total}` target=`{target}`",
                bucket = get(item, "bucket"),
                name = get(item, "name"),
                stories = gi(item, "story_count"),
                total = gi(item, "count"),
                target = get(item, "suggested_target"),
            ));
            let sample = get(item, "sample");
            if !sample.is_empty() {
                lines.push(format!("  样例：{sample}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Term Candidates".to_string());
    if !term_candidates.is_empty() {
        for item in term_candidates.iter().take(20) {
            lines.push(format!(
                "- `{bucket}::{name}` stories=`{stories}` total=`{total}` target=`{target}`",
                bucket = get(item, "bucket"),
                name = get(item, "name"),
                stories = gi(item, "story_count"),
                total = gi(item, "count"),
                target = get(item, "suggested_target"),
            ));
            let sample = get(item, "sample");
            if !sample.is_empty() {
                lines.push(format!("  样例：{sample}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Keep Candidates".to_string());
    if !keep_candidates.is_empty() {
        for item in keep_candidates.iter().take(12) {
            lines.push(format!(
                "- `{bucket}::{name}` stories=`{stories}` total=`{total}` target=`{target}`",
                bucket = get(item, "bucket"),
                name = get(item, "name"),
                stories = gi(item, "story_count"),
                total = gi(item, "count"),
                target = get(item, "suggested_target"),
            ));
            let reasons = item
                .get("reasons")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .take(2)
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            if !reasons.is_empty() {
                lines.push(format!("  说明：{reasons}"));
            }
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Deposition Targets".to_string());
    if !deposition_targets.is_empty() {
        for item in deposition_targets {
            lines.push(format!(
                "- `{}` x{}",
                get(item, "target"),
                gi(item, "count")
            ));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Writeback Queue".to_string());
    if !writeback_queue.is_empty() {
        for item in writeback_queue {
            lines.push(format!(
                "- `{kind}` `{name}` stories=`{stories}` total=`{total}` state=`{state}` -> `{target}`",
                kind = get(item, "kind"),
                name = get(item, "name"),
                stories = gi(item, "stories"),
                total = gi(item, "count"),
                state = get(item, "state"),
                target = get(item, "target"),
            ));
            lines.push(format!("  说明：{}", get(item, "reason")));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Next Actions".to_string());
    lines
        .push("1. 先看 `Merged Families` 和 `Writeback Queue`，避免同名异桶重复判断。".to_string());
    lines.push(
        "2. 再核 `Keep Candidates`，把设计性重复和局部节奏从纯负向规则里拆出来。".to_string(),
    );
    lines.push(
        "3. 最后按 `Deposition Targets` 分流到模板库、词库、评审指南或本书规则。".to_string(),
    );
    lines.push(String::new());

    lines.join("\n") + "\n"
}

/// `build_catalog_payload`。
pub fn build_catalog_payload(ctx: &DraftContext, novel_dir: &Path, payloads: &[Value]) -> Value {
    let draft = ctx.draft_rules();
    let template_bank_names = load_bank_names(draft, "name");
    let term_bank_names = load_bank_names(draft, "term");
    let builtin_template_names = load_builtin_rule_template_names(draft);
    let builtin_term_names = load_builtin_rule_term_names(draft);
    let template_candidates = aggregate_candidates(payloads, "template_bank_candidates");
    let template_families = aggregate_candidate_families(&template_candidates);
    let term_candidates = aggregate_candidates(payloads, "term_bank_candidates");
    let keep_candidates = aggregate_candidates(payloads, "keep_candidates");
    let deposition_targets = aggregate_deposition_targets(payloads);
    let learning_anchors =
        build_learning_anchors(&template_families, &term_candidates, &keep_candidates);
    let writeback_queue = build_writeback_queue(
        &template_families,
        &term_candidates,
        &keep_candidates,
        &template_bank_names,
        &term_bank_names,
        &builtin_template_names,
        &builtin_term_names,
    );
    let story_paths: Vec<Value> = payloads
        .iter()
        .map(|payload| json!(payload.get("story").and_then(Value::as_str).unwrap_or("")))
        .collect();
    let mut map = Map::new();
    map.insert(
        "novel".to_string(),
        json!(novel_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()),
    );
    map.insert("stories".to_string(), json!(payloads.len()));
    map.insert("story_paths".to_string(), Value::Array(story_paths));
    map.insert(
        "template_candidates".to_string(),
        Value::Array(template_candidates.clone()),
    );
    map.insert(
        "template_families".to_string(),
        Value::Array(template_families),
    );
    map.insert("term_candidates".to_string(), Value::Array(term_candidates));
    map.insert("keep_candidates".to_string(), Value::Array(keep_candidates));
    map.insert(
        "deposition_targets".to_string(),
        Value::Array(deposition_targets),
    );
    map.insert(
        "learning_anchors".to_string(),
        Value::Array(learning_anchors),
    );
    map.insert("writeback_queue".to_string(), Value::Array(writeback_queue));
    Value::Object(map)
}
