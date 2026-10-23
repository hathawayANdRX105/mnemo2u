//! 输入发现/语料清洗、跟踪词窗口与把字操作（对齐 iter_target_files / clean_corpus_text 等）。

use super::*;

// ---------------------------------------------------------------------------
// 跟踪词窗口 / 把字操作 / 句长画像

/// 跟踪词分类标签（对齐 `tracked_term_category_label`）。
fn tracked_term_category_label(category: &str) -> String {
    match category {
        "characters" => "人物名".into(),
        "places" => "地点名".into(),
        "devices" => "设备名".into(),
        "actions" => "动作短语".into(),
        "atmosphere" => "氛围词".into(),
        "style" => "意象词".into(),
        "learned_term" => "语料高频词".into(),
        other => other.to_string(),
    }
}

/// 跟踪词窗口建议（对齐 `suggest_tracked_term_window_action`）。
fn suggest_tracked_term_window_action(category: &str) -> String {
    match category {
        "characters" => "用称谓、站位、动作或视角入口替换连续点名。".into(),
        "places" => "换成具体空间部件、声音、光线或行动路径，不要连续报地点名。".into(),
        "devices" => "让设备通过状态变化、故障后果或人物反应出现，不要连续点屏幕/终端。".into(),
        "actions" => "把重复动作拆成目的、阻力和结果，或换成身体反应。".into(),
        "atmosphere" | "style" => "保留最有用的一处意象，其余改成可见场面变化。".into(),
        "learned_term" => {
            "先判断它是临时角色、物件还是概念；用称谓、位置、动作和后果分担点名。".into()
        }
        _ => "检查同一词是否在替代镜头调度；优先换成动作、物件或视角变化。".into(),
    }
}

/// `format_tracked_term_counts`：`词 x计数(分类标签)` 用分隔符连接。
pub(crate) fn format_tracked_term_counts(
    terms: &[TrackedTermWindowTerm],
    separator: &str,
) -> String {
    terms
        .iter()
        .map(|t| {
            format!(
                "{} x{}({})",
                t.term,
                t.count,
                tracked_term_category_label(&t.category)
            )
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// 语料学习词能否进入跟踪窗口（对齐 `is_learned_term_window_candidate`）。
fn is_learned_term_window_candidate(
    ctx: &DraftContext,
    term: &str,
    known: &std::collections::HashSet<String>,
) -> bool {
    if code_len(term) < 2 {
        return false;
    }
    let window = ctx.learned_window();
    if window
        .noise_prefixes
        .iter()
        .any(|p| term.starts_with(p.as_str()))
        || window
            .noise_suffixes
            .iter()
            .any(|s| term.ends_with(s.as_str()))
        || window.noise_chars.iter().any(|c| term.contains(c.as_str()))
    {
        return false;
    }
    for known_term in known {
        if !known_term.is_empty() && known_term != term && term.contains(known_term.as_str()) {
            return false;
        }
    }
    true
}

/// 跟踪词局部窗口（对齐 `build_tracked_term_windows`）。
pub(crate) fn build_tracked_term_windows(
    ctx: &DraftContext,
    term_bank: &[TrackedTerm],
    learned_terms: &[LearnedPattern],
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> Vec<TrackedTermWindow> {
    if sentence_infos.len() < 3 {
        return Vec::new();
    }
    let th = ctx.thresholds();
    let window_categories: std::collections::HashSet<&str> = ctx
        .learned_window()
        .categories
        .iter()
        .map(|c| c.as_str())
        .collect();
    let mut rules: Vec<(String, String, String)> = Vec::new();
    let mut seen_terms: std::collections::HashSet<String> = HashSet::new();
    for rule in term_bank {
        if rule.term.is_empty()
            || seen_terms.contains(&rule.term)
            || !window_categories.contains(rule.category.as_str())
        {
            continue;
        }
        rules.push((
            rule.term.clone(),
            rule.category.clone(),
            rule.note.clone().unwrap_or_default(),
        ));
        seen_terms.insert(rule.term.clone());
    }
    for rule in learned_terms {
        if rule.name.is_empty()
            || seen_terms.contains(&rule.name)
            || !window_categories.contains(rule.category.as_str())
            || !is_learned_term_window_candidate(ctx, &rule.name, &seen_terms)
        {
            continue;
        }
        rules.push((rule.name.clone(), rule.category.clone(), rule.note.clone()));
        seen_terms.insert(rule.name.clone());
    }
    if rules.is_empty() {
        return Vec::new();
    }

    let window_size = th.tracked_term_window_size as usize;
    let min_top = th.tracked_term_window_min_top as usize;
    let min_total = th.tracked_term_window_min_total as usize;
    let min_category = th.tracked_term_window_min_category as usize;

    let mut candidates: Vec<TrackedTermWindow> = Vec::new();
    for start in 0..sentence_infos
        .len()
        .saturating_sub(window_size)
        .saturating_add(1)
    {
        let chunk =
            &sentence_infos[start..start.saturating_add(window_size).min(sentence_infos.len())];
        if chunk.len() < 3 {
            continue;
        }
        let mut term_counts = Counter::default();
        let mut term_categories: HashMap<String, String> = HashMap::new();
        let mut term_notes: HashMap<String, String> = HashMap::new();
        for sentence in chunk {
            for (term, category, note) in &rules {
                let count = sentence.text.matches(term.as_str()).count();
                if count == 0 {
                    continue;
                }
                term_counts.add(term.as_str());
                term_categories
                    .entry(term.clone())
                    .or_insert_with(|| category.clone());
                term_notes
                    .entry(term.clone())
                    .or_insert_with(|| note.clone());
            }
        }
        if term_counts.is_empty() {
            continue;
        }
        let most = term_counts.most_common_all();
        let (top_term, top_count) = &most[0];
        let total_hits: usize = most.iter().map(|(_, c)| *c).sum();
        let mut category_counts = Counter::default();
        for (term, count) in &most {
            let category = term_categories
                .get(term)
                .cloned()
                .unwrap_or_else(|| "tracked".into());
            for _ in 0..*count {
                category_counts.add(&category);
            }
        }
        let top_cat = category_counts.most_common_all();
        let (top_category, top_category_count) = if top_cat.is_empty() {
            ("tracked".to_string(), 0usize)
        } else {
            (top_cat[0].0.clone(), top_cat[0].1)
        };

        let mut reasons: Vec<String> = Vec::new();
        if *top_count >= min_top {
            reasons.push(format!("同词 {top_term} x{top_count}/{}", chunk.len()));
        }
        if total_hits >= min_total {
            reasons.push(format!("跟踪词合计 {total_hits}/{}", chunk.len()));
        }
        if top_category_count >= min_category {
            reasons.push(format!(
                "{} x{}/{}",
                tracked_term_category_label(&top_category),
                top_category_count,
                chunk.len()
            ));
        }
        if reasons.is_empty() {
            continue;
        }
        let top_terms: Vec<TrackedTermWindowTerm> = term_counts
            .most_common(5)
            .into_iter()
            .map(|(term, count)| TrackedTermWindowTerm {
                category: term_categories
                    .get(&term)
                    .cloned()
                    .unwrap_or_else(|| "tracked".into()),
                note: term_notes.get(&term).cloned().unwrap_or_default(),
                term,
                count,
            })
            .collect();
        candidates.push(TrackedTermWindow {
            start_index: chunk[0].index,
            end_index: chunk[chunk.len() - 1].index,
            start_line: chunk[0].line_no,
            end_line: chunk[chunk.len() - 1].line_no,
            window_size: chunk.len(),
            score: top_count * 3 + total_hits + top_category_count,
            total_hits,
            top_term: top_term.clone(),
            top_count: *top_count,
            top_category: top_category.clone(),
            reasons,
            terms: top_terms,
            suggestion: suggest_tracked_term_window_action(&top_category),
            sample: chunk.iter().map(|i| i.text.clone()).collect(),
            total_candidates: 0,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.top_count.cmp(&a.top_count))
            .then(a.start_index.cmp(&b.start_index))
            .then_with(|| a.top_term.cmp(&b.top_term))
    });
    let total_candidates = candidates.len();
    let mut kept: Vec<TrackedTermWindow> = Vec::new();
    let mut kept_spans: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for candidate in candidates {
        let span = (candidate.start_index, candidate.end_index);
        let overlaps = kept_spans
            .get(&candidate.top_term)
            .is_some_and(|existing| existing.iter().any(|e| !(span.1 < e.0 || span.0 > e.1)));
        if overlaps {
            continue;
        }
        kept_spans
            .entry(candidate.top_term.clone())
            .or_default()
            .push(span);
        let mut window = candidate;
        window.total_candidates = total_candidates;
        kept.push(window);
        if kept.len() >= sample_limit {
            break;
        }
    }
    kept
}

/// 把字操作角色分类（对齐 `classify_ba_operation`）。
fn classify_ba_operation(ctx: &DraftContext, snippet: &str) -> &'static str {
    let lex = ctx.lexicon();
    if lex
        .ba_emotion_terms
        .iter()
        .any(|t| snippet.contains(t.as_str()))
    {
        return "情绪动作";
    }
    if lex
        .ba_clue_terms
        .iter()
        .any(|t| snippet.contains(t.as_str()))
    {
        return "线索操作";
    }
    if lex
        .ba_scene_terms
        .iter()
        .chain(lex.ba_scene_verbs.iter())
        .any(|t| snippet.contains(t.as_str()))
    {
        return "场面调度";
    }
    if lex
        .ba_tool_terms
        .iter()
        .any(|t| snippet.contains(t.as_str()))
    {
        return "工具操作";
    }
    "动作操作"
}

/// 把字操作建议（对齐 `suggest_ba_operation_action`）。
fn suggest_ba_operation_action(role: &str) -> String {
    match role {
        "工具操作" => "必要工具动作可保留，但连续出现时要补结果、阻力或人物反应。".into(),
        "线索操作" => "把线索操作拆成发现、误读、排除和后果，少写整理流程。".into(),
        "情绪动作" => "优先改成身体反应、声音变化或他人误读，不要只把情绪推来推去。".into(),
        "场面调度" => "保留能改变画面的句子，其余改成环境后果或视角移动。".into(),
        _ => "检查这个把字句是否只是操作日志；能换结果句、被动阻力或场面反馈就换。".into(),
    }
}

/// 把字操作语境（对齐 `build_ba_operation_contexts`）。
pub(crate) fn build_ba_operation_contexts(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> Vec<BaContext> {
    struct Bucket {
        count: usize,
        samples: Vec<BaSample>,
    }
    let mut order: Vec<&'static str> = Vec::new();
    let mut buckets: HashMap<&'static str, Bucket> = HashMap::new();
    let mut total = 0usize;
    for sentence in sentence_infos {
        for m in ctx
            .ba_regex
            .find_iter(&sentence.text)
            .filter_map(|m| m.ok())
        {
            let snippet = m.as_str().to_string();
            let role = classify_ba_operation(ctx, &snippet);
            let entry = buckets.entry(role).or_insert_with(|| {
                order.push(role);
                Bucket {
                    count: 0,
                    samples: Vec::new(),
                }
            });
            entry.count += 1;
            total += 1;
            if entry.samples.len() < sample_limit {
                entry.samples.push(BaSample {
                    index: sentence.index,
                    line_no: sentence.line_no,
                    snippet,
                    sentence: sentence.text.clone(),
                });
            }
        }
    }
    let role_order = |role: &str| -> u32 {
        match role {
            "线索操作" => 0,
            "情绪动作" => 1,
            "动作操作" => 2,
            "场面调度" => 3,
            "工具操作" => 4,
            _ => 99,
        }
    };
    let mut rows: Vec<BaContext> = Vec::new();
    for role in order {
        let bucket = &buckets[role];
        let warn = if matches!(role, "线索操作" | "情绪动作" | "动作操作") {
            bucket.count >= 2
        } else {
            bucket.count >= 4
        };
        rows.push(BaContext {
            role: role.to_string(),
            count: bucket.count,
            samples: bucket.samples.clone(),
            suggestion: suggest_ba_operation_action(role),
            warn,
            total,
        });
    }
    rows.sort_by(|a, b| {
        role_order(&a.role)
            .cmp(&role_order(&b.role))
            .then(b.count.cmp(&a.count))
    });
    rows
}

// ---------------------------------------------------------------------------
// 输入发现 / 语料清洗（对齐 iter_target_files / clean_corpus_text）

/// 判断路径是否属于生成物或模板目录（对齐 `_is_generated_or_template`）。
pub(crate) fn is_generated_or_template(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let has = |s: &str| names.iter().any(|n| n == s);
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    has("_templates")
        || has("draft-stats")
        || has("story-plan-stats")
        || has("chapter-plan-stats")
        || has("arc-plan-stats")
        || has("card-stats")
        || filename.to_uppercase().starts_with("README")
        || filename == "progression.md"
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// 去掉路径中的 `.`（CurrentDir）分量（路径归一化）
/// （如 "./a" → "a"；相对路径不带 "./" 前缀）。
fn normalize_current_dir(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// 展开输入路径为文件列表：目录递归收 `.md/.txt`
/// 且排除生成物/模板，显式文件原样保留，结果按路径排序；
/// 路径归一化去掉 `.` 分量。
pub fn iter_target_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in paths {
        let raw = normalize_current_dir(raw);
        if raw.is_dir() {
            let mut found = Vec::new();
            walk_files(&raw, &mut found);
            found.retain(|p| {
                p.extension()
                    .map(|e| e == "md" || e == "txt")
                    .unwrap_or(false)
                    && !is_generated_or_template(p)
            });
            let mut normalized: Vec<PathBuf> = found
                .into_iter()
                .map(|p| normalize_current_dir(&p))
                .collect();
            normalized.sort();
            out.extend(normalized);
        } else if raw.is_file() {
            out.push(raw);
        }
    }
    out
}

/// `re.fullmatch(pattern, text)`：`fancy-regex` 无 `is_whole_match`，用 `find` 起止判定。
pub(crate) fn is_whole_match(regex: &fancy_regex::Regex, text: &str) -> bool {
    regex
        .find(text)
        .ok()
        .flatten()
        .map(|m| m.start() == 0 && m.end() == text.len())
        .unwrap_or(false)
}

/// 语料清洗（对齐 `clean_corpus_text`）。
pub(crate) fn clean_corpus_text(ctx: &DraftContext, path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("无法读取语料文件 {}", path.display()))?;
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let stripped = line.trim();
        if stripped.is_empty() {
            continue;
        }
        if ctx
            .corpus_markdown_noise_regex
            .is_match(stripped)
            .unwrap_or(false)
            || is_whole_match(&ctx.corpus_table_line_regex, stripped)
        {
            continue;
        }
        let mut s = ctx
            .corpus_bullet_regex
            .replace_all(stripped, "")
            .into_owned();
        s = ctx.corpus_backtick_regex.replace_all(&s, "$1").into_owned();
        s = ctx.corpus_bold_regex.replace_all(&s, "$1").into_owned();
        if !s.is_empty() {
            lines.push(s);
        }
    }
    Ok(lines.join("\n"))
}
