//! 跨 story 候选/家族聚合、学习锚点与回写队列（供 `catalog::report` 共享）。

use super::*;

/// `bucket_family`：候选家族归并。
fn bucket_family(bucket: &str) -> String {
    if matches!(
        bucket,
        "custom_template" | "patterns" | "phrases" | "tokens"
    ) {
        "template_family".to_string()
    } else if matches!(
        bucket,
        "punctuation" | "sentence_length" | "fatigue_window" | "ba_operation_context"
    ) {
        "narrative_signal".to_string()
    } else if matches!(bucket, "learned_filter" | "tracked_term") {
        "term_family".to_string()
    } else {
        bucket.to_string()
    }
}

/// 中间行（聚合候选；行键序固定，`_story_set` 不序列化）。
#[derive(Debug, Default)]
struct CandidateRow {
    bucket: String,
    name: String,
    count: i64,
    story_count: i64,
    stories: Vec<String>,
    chapter_total: i64,
    sample: String,
    suggested_target: String,
    reasons: Vec<String>,
    story_set: HashSet<String>,
}

impl CandidateRow {
    fn to_value(&self) -> Value {
        json!({
            "bucket": self.bucket,
            "name": self.name,
            "count": self.count,
            "story_count": self.story_count,
            "stories": self.stories,
            "chapter_total": self.chapter_total,
            "sample": self.sample,
            "suggested_target": self.suggested_target,
            "reasons": self.reasons,
        })
    }
}

/// `aggregate_candidates(payloads, field, count_key="count")`。
pub(crate) fn aggregate_candidates(payloads: &[Value], field: &str) -> Vec<Value> {
    let mut rows: Vec<CandidateRow> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();

    for payload in payloads {
        let story = payload
            .get("story")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let chapters = payload.get("chapters").and_then(Value::as_i64).unwrap_or(0);
        let items = match payload.get(field).and_then(Value::as_array) {
            Some(items) => items,
            None => continue,
        };
        for item in items {
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let bucket = item
                .get("bucket")
                .and_then(Value::as_str)
                .unwrap_or(field)
                .to_string();
            let key = (bucket.clone(), name.clone());
            let row = match index.get(&key) {
                Some(&i) => &mut rows[i],
                None => {
                    let i = rows.len();
                    index.insert(key.clone(), i);
                    rows.push(CandidateRow {
                        bucket: bucket.clone(),
                        name: name.clone(),
                        count: 0,
                        story_count: 0,
                        stories: Vec::new(),
                        chapter_total: 0,
                        sample: item
                            .get("sample")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        suggested_target: item
                            .get("suggested_target")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        reasons: Vec::new(),
                        story_set: HashSet::new(),
                    });
                    &mut rows[i]
                }
            };
            row.count += item.get("count").and_then(Value::as_i64).unwrap_or(0);
            if row.story_set.insert(story.clone()) {
                row.chapter_total += chapters;
            }
            let reason = item
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !reason.is_empty() && !row.reasons.contains(&reason) {
                row.reasons.push(reason);
            }
            if row.sample.is_empty() {
                let sample = item
                    .get("sample")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !sample.is_empty() {
                    row.sample = sample;
                }
            }
        }
    }

    let mut rows: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            row.story_count = row.story_set.len() as i64;
            let mut stories: Vec<String> = row.story_set.iter().cloned().collect();
            stories.sort();
            row.stories = stories;
            row.to_value()
        })
        .collect();
    sort_by_counts(&mut rows);
    rows
}

/// `(-story_count, -count, name)` 的稳定排序
/// （无键时退回首现序）。
fn sort_by_counts(rows: &mut [Value]) {
    rows.sort_by(|a, b| {
        b.get("story_count")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .cmp(&a.get("story_count").and_then(Value::as_i64).unwrap_or(0))
            .then(
                b.get("count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .cmp(&a.get("count").and_then(Value::as_i64).unwrap_or(0)),
            )
            .then_with(|| {
                a.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
            })
    });
}

/// 中间行（家族聚合；行键序固定）。
#[derive(Debug, Default)]
struct FamilyRow {
    name: String,
    count: i64,
    story_count: i64,
    stories: Vec<String>,
    chapter_total: i64,
    sample: String,
    suggested_target: String,
    buckets: Vec<String>,
    bucket_families: Vec<String>,
    reasons: Vec<String>,
    story_set: HashSet<String>,
    bucket_set: HashSet<String>,
    family_set: HashSet<String>,
}

impl FamilyRow {
    fn to_value(&self) -> Value {
        json!({
            "name": self.name,
            "count": self.count,
            "story_count": self.story_count,
            "chapter_total": self.chapter_total,
            "sample": self.sample,
            "suggested_target": self.suggested_target,
            "reasons": self.reasons,
            "stories": self.stories,
            "buckets": self.buckets,
            "bucket_families": self.bucket_families,
        })
    }
}

/// `aggregate_candidate_families(candidates)`。
pub(crate) fn aggregate_candidate_families(candidates: &[Value]) -> Vec<Value> {
    let mut rows: Vec<FamilyRow> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();

    for item in candidates {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let row = match index.get(&name) {
            Some(&i) => &mut rows[i],
            None => {
                let i = rows.len();
                index.insert(name.clone(), i);
                rows.push(FamilyRow {
                    name: name.clone(),
                    count: 0,
                    story_count: 0,
                    stories: Vec::new(),
                    chapter_total: 0,
                    sample: item
                        .get("sample")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    suggested_target: item
                        .get("suggested_target")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    buckets: Vec::new(),
                    bucket_families: Vec::new(),
                    reasons: Vec::new(),
                    story_set: HashSet::new(),
                    bucket_set: HashSet::new(),
                    family_set: HashSet::new(),
                });
                &mut rows[i]
            }
        };
        row.count += item.get("count").and_then(Value::as_i64).unwrap_or(0);
        if let Some(stories) = item.get("stories").and_then(Value::as_array) {
            for story in stories {
                row.story_set
                    .insert(story.as_str().unwrap_or_default().to_string());
            }
        }
        row.chapter_total = row.chapter_total.max(
            item.get("chapter_total")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        );
        let bucket = item.get("bucket").and_then(Value::as_str).unwrap_or("");
        row.bucket_set.insert(bucket.to_string());
        row.family_set.insert(bucket_family(bucket));
        if let Some(reasons) = item.get("reasons").and_then(Value::as_array) {
            for reason in reasons {
                let text = reason.as_str().unwrap_or_default().trim().to_string();
                if !text.is_empty() && !row.reasons.contains(&text) {
                    row.reasons.push(text);
                }
            }
        }
        if row.sample.is_empty() {
            let sample = item
                .get("sample")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !sample.is_empty() {
                row.sample = sample;
            }
        }
    }

    let mut rows: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            row.story_count = row.story_set.len() as i64;
            let mut stories: Vec<String> = row.story_set.iter().cloned().collect();
            stories.sort();
            row.stories = stories;
            let mut buckets: Vec<String> = row.bucket_set.iter().cloned().collect();
            buckets.sort();
            row.buckets = buckets;
            let mut families: Vec<String> = row.family_set.iter().cloned().collect();
            families.sort();
            row.bucket_families = families;
            row.to_value()
        })
        .collect();
    sort_by_counts(&mut rows);
    rows
}

/// `aggregate_deposition_targets(payloads)`（全量计数）。
pub(crate) fn aggregate_deposition_targets(payloads: &[Value]) -> Vec<Value> {
    let mut counter: Ctr = Ctr::default();
    for payload in payloads {
        let items = match payload.get("deposition_targets").and_then(Value::as_array) {
            Some(items) => items,
            None => continue,
        };
        for item in items {
            let target = item
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let count = item.get("count").and_then(Value::as_i64).unwrap_or(0);
            counter.add(&target, count as usize);
        }
    }
    counter
        .most_common_all()
        .into_iter()
        .map(|(target, count)| json!({"target": target, "count": count}))
        .collect()
}

/// `build_learning_anchors`（排序后截前 16）。
pub(crate) fn build_learning_anchors(
    template_families: &[Value],
    term_candidates: &[Value],
    keep_candidates: &[Value],
) -> Vec<Value> {
    let mut anchors: Vec<Value> = Vec::new();
    for item in template_families {
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        let count = item.get("count").and_then(Value::as_i64).unwrap_or(0);
        if story_count >= 2 && count >= 10 {
            let buckets = item
                .get("buckets")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .map(|b| b.as_str().unwrap_or_default().to_string())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            anchors.push(json!({
                "kind": "template",
                "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                "bucket": buckets,
                "count": count,
                "story_count": story_count,
                "suggested_target": item.get("suggested_target").and_then(Value::as_str).unwrap_or(""),
                "reason": "跨 Story 反复出现，值得判断它是硬模板、结构模式，还是应改抽取定义。",
            }));
        }
    }
    for item in term_candidates {
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        let count = item.get("count").and_then(Value::as_i64).unwrap_or(0);
        if story_count >= 2 && count >= 8 {
            anchors.push(json!({
                "kind": "term",
                "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                "bucket": item.get("bucket").and_then(Value::as_str).unwrap_or(""),
                "count": count,
                "story_count": story_count,
                "suggested_target": item.get("suggested_target").and_then(Value::as_str).unwrap_or(""),
                "reason": "跨 Story 稳定偏高，适合正式进词库或改成更细的 phrase 规则。",
            }));
        }
    }
    for item in keep_candidates {
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        if story_count >= 2 {
            anchors.push(json!({
                "kind": "keep",
                "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                "bucket": item.get("bucket").and_then(Value::as_str).unwrap_or(""),
                "count": item.get("count").and_then(Value::as_i64).unwrap_or(0),
                "story_count": story_count,
                "suggested_target": item.get("suggested_target").and_then(Value::as_str).unwrap_or(""),
                "reason": "多条 Story 都在把它当加分或保留候选，说明审查不该一刀切地误杀这类文笔设计。",
            }));
        }
    }
    sort_by_counts(&mut anchors);
    anchors.truncate(16);
    anchors
}

/// `build_writeback_queue`（排序 `(-stories, -count, name)` 后截前 16）。
pub(crate) fn build_writeback_queue(
    template_families: &[Value],
    term_candidates: &[Value],
    keep_candidates: &[Value],
    template_bank_names: &HashSet<String>,
    term_bank_names: &HashSet<String>,
    builtin_template_names: &HashSet<String>,
    builtin_term_names: &HashSet<String>,
) -> Vec<Value> {
    let mut queue: Vec<Value> = Vec::new();
    for item in template_families {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let in_bank = template_bank_names.contains(&name);
        let in_builtin = builtin_template_names.contains(&name);
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        let count = item.get("count").and_then(Value::as_i64).unwrap_or(0);
        if story_count < 2 {
            continue;
        }
        if in_bank && count >= 12 {
            queue.push(json!({
                "kind": "template_recalibration",
                "name": name,
                "target": "configs/rules/review.yaml#draft.template_rules",
                "stories": story_count,
                "count": count,
                "state": "bank",
                "reason": "模板已在库中，但跨 Story 仍高频命中，应回看 pattern、阈值或说明是否过宽。",
            }));
        } else if in_builtin && count >= 12 {
            queue.push(json!({
                "kind": "rule_recalibration",
                "name": name,
                "target": "audit.draft",
                "stories": story_count,
                "count": count,
                "state": "builtin",
                "reason": "这条规则已经写在审查脚本里，高频命中更像阈值、分类或说明需要回调，而不是简单再加一条 bank。",
            }));
        } else if !in_bank && count >= 12 {
            queue.push(json!({
                "kind": "template_add",
                "name": name,
                "target": "configs/rules/review.yaml#draft.template_rules",
                "stories": story_count,
                "count": count,
                "state": "new",
                "reason": "同名家族跨 Story 稳定出现，适合进入模板库或至少进入候选审阅清单。",
            }));
        }
    }
    for item in term_candidates {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let in_bank = term_bank_names.contains(&name);
        let in_builtin = builtin_term_names.contains(&name);
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        let count = item.get("count").and_then(Value::as_i64).unwrap_or(0);
        if story_count < 2 {
            continue;
        }
        if in_bank && count >= 12 {
            queue.push(json!({
                "kind": "term_recalibration",
                "name": name,
                "target": "configs/rules/review.yaml#draft.tracked_terms",
                "stories": story_count,
                "count": count,
                "state": "bank",
                "reason": "词项已在库中却仍跨 Story 偏高，应调阈值、说明，或拆成更细 phrase 规则。",
            }));
        } else if in_builtin && count >= 8 {
            queue.push(json!({
                "kind": "rule_recalibration",
                "name": name,
                "target": "audit.draft",
                "stories": story_count,
                "count": count,
                "state": "builtin",
                "reason": "这条词项已经在审查脚本基础规则里，高频命中说明更适合调阈值或拆分类，而不是重复入库。",
            }));
        } else if !in_bank && count >= 8 {
            queue.push(json!({
                "kind": "term_add",
                "name": name,
                "target": "configs/rules/review.yaml#draft.tracked_terms",
                "stories": story_count,
                "count": count,
                "state": "new",
                "reason": "词项跨 Story 稳定偏高，适合做第一轮真实学习回写。",
            }));
        }
    }
    for item in keep_candidates {
        let story_count = item.get("story_count").and_then(Value::as_i64).unwrap_or(0);
        if story_count >= 3 {
            queue.push(json!({
                "kind": "keep_rule",
                "name": item.get("name").and_then(Value::as_str).unwrap_or(""),
                "target": "skills/review-guide.md",
                "stories": story_count,
                "count": item.get("count").and_then(Value::as_i64).unwrap_or(0),
                "state": "new",
                "reason": "这类候选在多条 Story 都被视作可保留，应补“设计性重复保留”口径。",
            }));
        }
    }
    // 排序键 (-stories, -count, name)：writeback 行没有 story_count 键位，改读 "stories"。
    queue.sort_by(|a, b| {
        b.get("stories")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .cmp(&a.get("stories").and_then(Value::as_i64).unwrap_or(0))
            .then(
                b.get("count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .cmp(&a.get("count").and_then(Value::as_i64).unwrap_or(0)),
            )
            .then_with(|| {
                a.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
            })
    });
    queue.truncate(16);
    queue
}
