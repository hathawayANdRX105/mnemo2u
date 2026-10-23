//! 句长画像、句首/分句骨架、ngram 与语料画像（对齐 build_sentence_length_profile / build_corpus_profile 等）。

use super::*;

/// 百分位（对齐 `_percentile`：半偶取整下标）。
fn percentile(values: &[usize], pct: f64) -> usize {
    if values.is_empty() {
        return 0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let index = bankers_round_int((ordered.len() - 1) as f64 * pct).max(0) as usize;
    ordered[index.min(ordered.len() - 1)]
}

fn flush_short_run(
    ctx: &DraftContext,
    run_min: usize,
    runs: &mut Vec<ShortRun>,
    current: &mut Vec<&crate::text::SentenceInfo>,
) {
    if current.len() >= run_min {
        let roles = summarize_short_roles(ctx, &current[..]);
        let total: usize = current.iter().map(|i| i.chars).sum();
        let run = ShortRun {
            start_index: current[0].index,
            end_index: current[current.len() - 1].index,
            start_line: current[0].line_no,
            end_line: current[current.len() - 1].line_no,
            avg_chars: round2(total as f64 / current.len() as f64),
            roles: roles.clone(),
            suggestion: suggest_short_run_action(&roles),
            sample: current.iter().take(5).map(|i| i.text.clone()).collect(),
        };
        runs.push(run);
    }
    current.clear();
}

/// 句长画像（对齐 `build_sentence_length_profile`）。
pub(crate) fn build_sentence_length_profile(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
) -> SentenceLengths {
    let th = ctx.thresholds();
    let short_max = th.short_sentence_max_chars as usize;
    let very_short_max = th.very_short_sentence_max_chars as usize;
    let run_max = th.short_sentence_run_max_chars as usize;
    let run_min = th.short_sentence_run_min as usize;

    let lengths: Vec<usize> = sentence_infos.iter().map(|i| i.chars).collect();
    let short_items: Vec<&crate::text::SentenceInfo> = sentence_infos
        .iter()
        .filter(|i| i.chars <= short_max)
        .collect();
    let very_short_items: Vec<&crate::text::SentenceInfo> = sentence_infos
        .iter()
        .filter(|i| i.chars <= very_short_max)
        .collect();

    let mut runs: Vec<ShortRun> = Vec::new();
    let mut current: Vec<&crate::text::SentenceInfo> = Vec::new();
    let mut previous_index = 0usize;
    for item in sentence_infos {
        if item.chars <= run_max && (current.is_empty() || item.index == previous_index + 1) {
            current.push(item);
        } else {
            flush_short_run(ctx, run_min, &mut runs, &mut current);
            if item.chars <= run_max {
                current.push(item);
            }
        }
        previous_index = item.index;
    }
    flush_short_run(ctx, run_min, &mut runs, &mut current);

    let short_ratio = short_items.len() as f64 / sentence_infos.len().max(1) as f64;
    let warn = very_short_items.len() >= 3 || !runs.is_empty() || short_ratio >= 0.18;
    SentenceLengths {
        count: sentence_infos.len(),
        min_chars: lengths.iter().min().copied().unwrap_or(0),
        p10_chars: percentile(&lengths, 0.10),
        p25_chars: percentile(&lengths, 0.25),
        median_chars: percentile(&lengths, 0.50),
        avg_chars: round2(
            lengths.iter().sum::<usize>() as f64 / sentence_infos.len().max(1) as f64,
        ),
        max_chars: lengths.iter().max().copied().unwrap_or(0),
        short_count: short_items.len(),
        very_short_count: very_short_items.len(),
        short_ratio: round4f(short_ratio),
        warn,
        short_sentences: short_items
            .iter()
            .take(20)
            .map(|i| ShortSentence {
                index: i.index,
                line_no: i.line_no,
                chars: i.chars,
                text: i.text.clone(),
            })
            .collect(),
        very_short_sentences: very_short_items
            .iter()
            .take(20)
            .map(|i| ShortSentence {
                index: i.index,
                line_no: i.line_no,
                chars: i.chars,
                text: i.text.clone(),
            })
            .collect(),
        short_runs: runs.into_iter().take(10).collect(),
        sentences: sentence_infos
            .iter()
            .map(|i| ShortSentence {
                index: i.index,
                line_no: i.line_no,
                chars: i.chars,
                text: i.text.clone(),
            })
            .collect(),
    }
}

/// ngram 高频词（对齐 `collect_ngram_terms` 新算法：`sizes` 预排序
/// （`tuple(sorted(min_count_by_size))`）扫描；`one_terms` 预计算；
/// 去重 = 已收短语的 `covered_phrases` 子串覆盖集（size ∈ sizes 且
/// `counts.get(sub, 0) <= count`），平手按首现序）。
pub fn collect_ngram_terms(
    ctx: &DraftContext,
    text: &str,
    min_count_by_size: &[(usize, usize)],
    require_structure: bool,
) -> Vec<(String, usize)> {
    let cleaned: String = ctx.ngram_keep_regex.replace_all(text, "").into_owned();
    let chars: Vec<char> = cleaned.chars().collect();
    let n = chars.len();
    let mut counts = Counter::default();
    let word_stoplist: std::collections::HashSet<&str> = ctx
        .lexicon()
        .word_stoplist
        .iter()
        .map(|s| s.as_str())
        .collect();
    let structure_chars: std::collections::HashSet<char> =
        ctx.lexicon().structure_chars.chars().collect();
    // `sizes = tuple(sorted(min_count_by_size))` 与 `one_terms` 预计算（循环外）。
    let mut sizes: Vec<(usize, usize)> = min_count_by_size.to_vec();
    sizes.sort();
    let one_terms: std::collections::HashMap<usize, String> = sizes
        .iter()
        .map(|(size, _)| (*size, "一".repeat(*size)))
        .collect();
    for (size, _min) in &sizes {
        if n < *size {
            continue;
        }
        for idx in 0..=n - size {
            let phrase: String = chars[idx..idx + size].iter().collect();
            if word_stoplist.contains(phrase.as_str()) {
                continue;
            }
            if is_whole_match(&ctx.ascii_alpha_regex, &phrase) {
                continue;
            }
            if phrase == one_terms[size] {
                continue;
            }
            if require_structure
                && !chars[idx..idx + size]
                    .iter()
                    .any(|c| structure_chars.contains(c))
            {
                continue;
            }
            counts.add(&phrase);
        }
    }
    let min_for = |len: usize| -> usize {
        min_count_by_size
            .iter()
            .find(|(size, _)| *size == len)
            .map(|(_, min)| *min)
            .unwrap_or(99)
    };
    let mut filtered: Vec<(String, usize)> = counts
        .entries()
        .iter()
        .filter(|(phrase, count)| *count >= min_for(code_len(phrase)))
        .map(|(phrase, count)| (phrase.clone(), *count))
        .collect();
    filtered.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(code_len(&b.0).cmp(&code_len(&a.0)))
            .then_with(|| a.0.cmp(&b.0))
    });
    // 去重：保留短语时预标记其子串（`size <= phrase_len` 的 substring，
    // `counts.get(sub, 0) <= count`）进 `covered_phrases`；后续命中即跳过。
    let mut deduped: Vec<(String, usize)> = Vec::new();
    let mut covered_phrases: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (phrase, count) in filtered {
        if covered_phrases.contains(&phrase) {
            continue;
        }
        let phrase_len = code_len(&phrase);
        let pchars: Vec<char> = phrase.chars().collect();
        deduped.push((phrase, count));
        for (size, _min) in &sizes {
            if *size > phrase_len {
                continue;
            }
            for idx in 0..=phrase_len - size {
                let sub: String = pchars[idx..idx + size].iter().collect();
                if counts.get(&sub) <= count {
                    covered_phrases.insert(sub);
                }
            }
        }
    }
    deduped
}

/// 语料词是否有用（对齐 `_is_useful_corpus_term`）。
fn is_useful_corpus_term(ctx: &DraftContext, term: &str, category: &str) -> bool {
    let lex = ctx.lexicon();
    if lex.corpus_stop_terms.iter().any(|t| t == term) {
        return false;
    }
    if code_len(term) < 2 {
        return false;
    }
    if is_whole_match(&ctx.alpha_numeric_regex, term) {
        return false;
    }
    if !term.is_empty() && term.chars().all(|c| c == term.chars().next().unwrap()) {
        return false;
    }
    !(category == "learned_term"
        && code_len(term) == 2
        && !lex.allowed_short_corpus_terms.iter().any(|t| t == term))
}

/// 由 ngram 词表生成学习模式（对齐 `_learned_patterns_from_terms`，截到 learned_filter_limit）。
fn learned_patterns_from_terms(
    ctx: &DraftContext,
    category: &str,
    raw_terms: &[(String, usize)],
    corpus_chars: usize,
    min_per_10k_floor: f64,
    multiplier: f64,
    note: &str,
) -> Vec<LearnedPattern> {
    let limit = ctx.thresholds().learned_filter_limit as usize;
    let mut patterns: Vec<LearnedPattern> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (term, count) in raw_terms {
        if seen.contains(term.as_str()) || !is_useful_corpus_term(ctx, term, category) {
            continue;
        }
        let corpus_per_10k = density(*count, corpus_chars);
        patterns.push(LearnedPattern {
            category: category.to_string(),
            name: term.clone(),
            count: *count,
            corpus_per_10k: round2(corpus_per_10k),
            max_per_10k: round2((corpus_per_10k * multiplier).max(min_per_10k_floor)),
            note: note.to_string(),
        });
        seen.insert(term.as_str());
        if patterns.len() >= limit {
            break;
        }
    }
    patterns
}

/// 语料画像（对齐 `build_corpus_profile`；无可用语料时返回 None）。
///
/// `exclude` 中的文件（通常是本次分析目标自身）不计入语料：否则「语料学到
/// 的高频词」会包含被分析章节自己的用词，本章高频词必被本章判过密，
/// 指标自我实现、恒响。
pub fn build_corpus_profile(
    ctx: &DraftContext,
    paths: &[PathBuf],
    exclude: &[PathBuf],
) -> Result<Option<CorpusProfile>> {
    let excluded: std::collections::HashSet<PathBuf> = exclude
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .collect();
    let files: Vec<PathBuf> = iter_target_files(paths)
        .into_iter()
        .filter(|p| {
            p.extension()
                .map(|e| e == "md" || e == "txt")
                .unwrap_or(false)
                && !is_generated_or_template(p)
                && !excluded.contains(&p.canonicalize().unwrap_or_else(|_| p.clone()))
        })
        .collect();
    if files.is_empty() {
        return Ok(None);
    }
    let mut all_text_parts: Vec<String> = Vec::new();
    let mut draft_text_parts: Vec<String> = Vec::new();
    for path in &files {
        let cleaned = clean_corpus_text(ctx, path)?;
        if cleaned.is_empty() {
            continue;
        }
        all_text_parts.push(cleaned.clone());
        if ctx.paths().drafts_dir_component(path).is_some() {
            draft_text_parts.push(cleaned);
        }
    }
    let all_text = all_text_parts.join("\n\n");
    let draft_text = if draft_text_parts.is_empty() {
        all_text.clone()
    } else {
        draft_text_parts.join("\n\n")
    };
    let corpus_chars = all_text.chars().filter(|c| *c != '\n').count();
    let draft_chars = draft_text.chars().filter(|c| *c != '\n').count();
    if corpus_chars == 0 {
        return Ok(None);
    }
    let raw_terms = collect_ngram_terms(ctx, &all_text, &[(2, 30), (3, 18), (4, 12)], false);
    let mut style_raw_terms =
        collect_ngram_terms(ctx, &draft_text, &[(2, 24), (3, 14), (4, 10)], true);
    let style_chars: std::collections::HashSet<char> = "得像把还说看没不只更在就".chars().collect();
    style_raw_terms.retain(|(term, _)| term.chars().any(|c| style_chars.contains(&c)));
    let learned_terms = learned_patterns_from_terms(
        ctx,
        "learned_term",
        &raw_terms,
        corpus_chars,
        10.0,
        1.25,
        "从卡片/大纲/草稿语料学到的高频实体或动作词，当前章过线时要查是否点名过密",
    );
    let learned_style_phrases = learned_patterns_from_terms(
        ctx,
        "learned_style_phrase",
        &style_raw_terms,
        std::cmp::max(draft_chars, 1),
        5.0,
        1.15,
        "从现有草稿学到的高频句法手势，当前章过线时优先改写",
    );
    let draft_sentences = ctx.splitter().split_sentences(&draft_text);
    let limit = ctx.thresholds().learned_filter_limit as usize;
    let sentence_leads: Vec<LearnedSentenceLead> = collect_sentence_starts(&draft_sentences)
        .into_iter()
        .filter(|(phrase, count)| {
            *count >= 6
                && !ctx
                    .corpus_markdown_noise_regex
                    .is_match(phrase)
                    .unwrap_or(false)
        })
        .map(|(phrase, count)| LearnedSentenceLead {
            phrase,
            count,
            corpus_per_10k: round2(density(count, draft_chars.max(1))),
        })
        .take(limit)
        .collect();
    let aa_bb_shapes: Vec<LearnedAaBbShape> = collect_aa_bb_patterns(ctx, &draft_sentences, 1)
        .into_iter()
        .filter(|item| item.count >= 2)
        .take(limit)
        .map(|item| LearnedAaBbShape {
            name: item.name,
            count: item.count,
            note: item.note,
        })
        .collect();
    let baseline_profile =
        build_sentence_length_profile(ctx, &ctx.splitter().split_sentence_infos(&draft_text));
    Ok(Some(CorpusProfile {
        source_count: files.len(),
        chars: corpus_chars,
        draft_chars,
        learned_terms,
        learned_style_phrases,
        learned_sentence_leads: sentence_leads,
        learned_aa_bb_shapes: aa_bb_shapes,
        sentence_length_baseline: Some(SentenceLengthBaseline {
            sentence_count: baseline_profile.count,
            p10_chars: baseline_profile.p10_chars,
            p25_chars: baseline_profile.p25_chars,
            median_chars: baseline_profile.median_chars,
            avg_chars: baseline_profile.avg_chars,
            short_ratio: baseline_profile.short_ratio,
        }),
    }))
}

/// 语料学习词/句法手势的当前章指标（对齐 `build_learned_filter_metrics`：
/// 计数 ≥2 才纳入；排序 = warn 优先、计数降序、category/name 升序）。
pub fn build_learned_filter_metrics(
    corpus: Option<&CorpusProfile>,
    lines: &[String],
    chars: usize,
    sample_limit: usize,
) -> Vec<LearnedFilterMetric> {
    let Some(corpus) = corpus else {
        return Vec::new();
    };
    let mut metrics: Vec<LearnedFilterMetric> = Vec::new();
    for rule in corpus
        .learned_terms
        .iter()
        .chain(corpus.learned_style_phrases.iter())
    {
        let escaped = fancy_regex::escape(&rule.name);
        let Ok(re) = fancy_regex::Regex::new(&escaped) else {
            continue;
        };
        let (count, hits) = crate::rules::find_hits(&re, lines, sample_limit);
        if count < 2 {
            continue;
        }
        let per_10k = crate::rules::density(count, chars);
        let flag = count >= 2 && per_10k > rule.max_per_10k;
        metrics.push(LearnedFilterMetric {
            category: rule.category.clone(),
            name: rule.name.clone(),
            count,
            per_10k: round2(per_10k),
            corpus_per_10k: rule.corpus_per_10k,
            max_per_10k: rule.max_per_10k,
            note: rule.note.clone(),
            warn: flag,
            samples: hits,
        });
    }
    metrics.sort_by(|a, b| {
        b.warn
            .cmp(&a.warn)
            .then(b.count.cmp(&a.count))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.name.cmp(&b.name))
    });
    metrics
}

/// 句首短语（对齐 `leading_phrase`：剥前缀后取前 max_len 码点）。
fn leading_phrase(sentence: &str, max_len: usize) -> String {
    prefix_chars(lstrip_chars(sentence, LEADING_PUNCT), max_len)
}

/// 重复句首（对齐 `collect_sentence_starts`）。
pub(crate) fn collect_sentence_starts(sentences: &[String]) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for sentence in sentences {
        let lead = leading_phrase(sentence, 8);
        if code_len(&lead) < 2 {
            continue;
        }
        counts.add(&lead);
    }
    counts.most_common_all()
}

/// 主语起手（对齐 `collect_subject_leads`，≥3 才保留）。
pub(crate) fn collect_subject_leads(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for sentence in sentences {
        let lead = lstrip_chars(sentence, LEADING_PUNCT);
        for candidate in &ctx.lexicon().subject_leads {
            if lead.starts_with(candidate.as_str()) {
                counts.add(candidate);
                break;
            }
        }
    }
    counts
        .most_common_all()
        .into_iter()
        .filter(|(_, c)| *c >= 3)
        .collect()
}

/// 段首起手（对齐 `collect_paragraph_leads`，≥3 才保留）。
pub(crate) fn collect_paragraph_leads(
    ctx: &DraftContext,
    paragraphs: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for paragraph in paragraphs {
        let lead = lstrip_chars(paragraph, LEADING_PUNCT);
        for candidate in &ctx.lexicon().paragraph_leads {
            if lead.starts_with(candidate.as_str()) {
                counts.add(candidate);
                break;
            }
        }
    }
    counts
        .most_common_all()
        .into_iter()
        .filter(|(_, c)| *c >= 3)
        .collect()
}

/// 连接词句首（对齐 `collect_connective_sentence_patterns`）。
pub(crate) fn collect_connective_sentence_patterns(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for sentence in sentences {
        let lead = lstrip_chars(sentence, LEADING_PUNCT);
        for (label, regex) in &ctx.connective_patterns {
            if regex.find(lead).ok().flatten().is_some() {
                counts.add(label);
                break;
            }
        }
    }
    counts.most_common_all()
}

/// 分句骨架（对齐 `collect_clause_prefixes`，取分句前 4 码点，≥4 才保留）。
pub(crate) fn collect_clause_prefixes(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    let attribution = ctx.learned_window().attribution_suffixes.as_slice();
    for sentence in sentences {
        for clause in split_clauses(ctx, sentence) {
            let lead = lstrip_chars(&clause, LEADING_PUNCT);
            if code_len(lead) < 2 {
                continue;
            }
            // 话语归因（「段誉道」「他说」）在各语域都是正常小说设备，矩阵显示
            // 人写经典与 AI 章的出现占比相近、无判别力，却是人写长篇 learned
            // 标记的主要来源，故不进分句骨架重复统计。
            if attribution.iter().any(|s| lead.ends_with(s.as_str())) {
                continue;
            }
            counts.add(&prefix_chars(lead, 4));
        }
    }
    counts
        .most_common_all()
        .into_iter()
        .filter(|(_, c)| *c >= 4)
        .collect()
}

/// 分句切分（`CLAUSE_SPLIT`：`[，；：]`）。
fn split_clauses(ctx: &DraftContext, sentence: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut last = 0usize;
    for m in ctx
        .clause_split_regex
        .find_iter(sentence)
        .filter_map(|m| m.ok())
    {
        out.push(sentence[last..m.start()].to_string());
        last = m.end();
    }
    out.push(sentence[last..].to_string());
    out
}

/// 相邻分句骨架对（对齐 `collect_parallel_clauses`，≥4 才保留）。
pub(crate) fn collect_parallel_clauses(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for sentence in sentences {
        if !sentence.contains('，') {
            continue;
        }
        let clauses: Vec<String> = split_clauses(ctx, sentence)
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        for pair in clauses.windows(2) {
            let left = &pair[0];
            let right = &pair[1];
            let left_lead = prefix_chars(left, 2);
            let right_lead = prefix_chars(right, 2);
            if code_len(&left_lead) < 2 || code_len(&right_lead) < 2 {
                continue;
            }
            counts.add(&format!("{left_lead}/{right_lead}"));
        }
    }
    counts
        .most_common_all()
        .into_iter()
        .filter(|(_, c)| *c >= 4)
        .collect()
}

/// 修饰/动作词压力（对齐 `collect_modifier_pressure`）。
pub(crate) fn collect_modifier_pressure(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<ModifierPressure> {
    let lex = ctx.lexicon();
    let mut findings: Vec<ModifierPressure> = Vec::new();
    for (label, hints) in [
        ("形容词提示", &lex.adjective_hints),
        ("动词提示", &lex.verb_hints),
    ] {
        let mut total = 0usize;
        let mut hit_sentences = 0usize;
        for sentence in sentences {
            let count = hints
                .iter()
                .map(|h| sentence.matches(h.as_str()).count())
                .sum::<usize>();
            total += count;
            if count >= 3 {
                hit_sentences += 1;
            }
        }
        if total > 0 {
            findings.push(ModifierPressure {
                label: label.to_string(),
                total,
                dense_sentences: hit_sentences,
                warn: hit_sentences >= 4,
            });
        }
    }
    findings
}

/// 判断句收束（对齐 `collect_judgement_endings`，≥2 才保留）。
pub(crate) fn collect_judgement_endings(
    ctx: &DraftContext,
    sentences: &[String],
) -> Vec<(String, usize)> {
    let mut counts = Counter::default();
    for sentence in sentences {
        let stripped = sentence.trim();
        for ending in &ctx.lexicon().judgement_endings {
            if stripped.ends_with(ending.as_str()) {
                counts.add(ending);
                break;
            }
        }
    }
    counts
        .most_common_all()
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .collect()
}

/// AA/BB/重叠词模式（对齐 `collect_aa_bb_patterns`）。
pub(crate) fn collect_aa_bb_patterns(
    ctx: &DraftContext,
    sentences: &[String],
    sample_limit: usize,
) -> Vec<AaBbPattern> {
    let mut balanced_samples: Vec<(String, Vec<String>)> = Vec::new();
    let mut redup_counts = Counter::default();
    let mut redup_samples: Vec<(String, Vec<String>)> = Vec::new();
    for sentence in sentences {
        let stripped = sentence.trim();
        if stripped.is_empty() || code_len(stripped) > 120 {
            continue;
        }
        for m in ctx.aa_bb_regex.find_iter(stripped).filter_map(|m| m.ok()) {
            let token = m.as_str().to_string();
            redup_counts.add(&token);
            let entry = redup_samples.iter_mut().find(|(name, _)| name == &token);
            if let Some(e) = entry {
                if e.1.len() < sample_limit {
                    e.1.push(stripped.to_string());
                }
            } else {
                redup_samples.push((
                    token,
                    if sample_limit > 0 {
                        vec![stripped.to_string()]
                    } else {
                        Vec::new()
                    },
                ));
            }
        }
        if !stripped.contains('，') {
            continue;
        }
        let clauses: Vec<String> = split_clauses(ctx, stripped)
            .into_iter()
            .map(|c| strip_chars(&c, LEADING_PUNCT).to_string())
            .filter(|c| !c.is_empty())
            .collect();
        if clauses.len() < 3 {
            continue;
        }
        let clause_lengths: Vec<usize> = clauses.iter().map(|c| prose_char_count(c)).collect();
        for start in 0..clauses.len().saturating_sub(2) {
            for end in (start + 3)..=clauses.len().min(start + 5) {
                let window = &clause_lengths[start..end];
                let lo = window.iter().min().copied().unwrap_or(0);
                let hi = window.iter().max().copied().unwrap_or(0);
                if lo < 2 || hi > 10 || hi - lo > 2 {
                    continue;
                }
                let shape = window
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                let label = format!("短分句排比 {shape}");
                let entry = balanced_samples.iter_mut().find(|(name, _)| name == &label);
                if let Some(e) = entry {
                    if e.1.len() < sample_limit {
                        e.1.push(stripped.to_string());
                    }
                } else {
                    balanced_samples.push((
                        label,
                        if sample_limit > 0 {
                            vec![stripped.to_string()]
                        } else {
                            Vec::new()
                        },
                    ));
                }
                break;
            }
        }
    }
    let mut findings: Vec<AaBbPattern> = Vec::new();
    {
        let mut sorted = balanced_samples.clone();
        sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        for (label, samples) in sorted {
            findings.push(AaBbPattern {
                pattern_type: "balanced_clauses".into(),
                name: label,
                count: samples.len(),
                note: "AA/BB式短分句排比，密集时会把画面写成清单".into(),
                warn: samples.len() >= 2,
                samples,
            });
        }
    }
    for (token, count) in redup_counts.most_common(12) {
        let samples = redup_samples
            .iter()
            .find(|(name, _)| name == &token)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        findings.push(AaBbPattern {
            pattern_type: "reduplicative_word".into(),
            name: token,
            count,
            note: "重叠词节奏，重复后会暴露手癖".into(),
            warn: count >= 4,
            samples,
        });
    }
    findings
}
