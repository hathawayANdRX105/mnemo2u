//! 对白判定/短句角色、对白轴/判断词、对白检测与情绪/角色声音（对齐 is_dialogue_like 等）。

use super::*;

// ---------------------------------------------------------------------------
// 对白判定与短句角色（对齐 is_dialogue_like / classify_short_sentence_role 等）

/// 是否对白句（对齐 `is_dialogue_like`）。
fn is_dialogue_like(sentence: &str) -> bool {
    let raw = sentence.trim();
    raw.starts_with('“')
        || raw.starts_with('”')
        || raw.starts_with('"')
        || raw.starts_with("【Pi】")
        || raw.starts_with('「')
        || raw.starts_with('『')
        || raw.contains("：“")
        || prefix_chars(raw, 8).contains('】')
}

/// 短句角色分类（对齐 `classify_short_sentence_role`）。
fn classify_short_sentence_role(ctx: &DraftContext, sentence: &str) -> &'static str {
    let raw = sentence.trim();
    let stripped = lstrip_chars(raw, LEADING_PUNCT);
    let lex = ctx.lexicon();
    if is_dialogue_like(raw) {
        return "对白";
    }
    if lex
        .judgement_context_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
        || lex
            .judgement_endings
            .iter()
            .any(|e| stripped.ends_with(e.as_str()))
    {
        return "判断";
    }
    if lex
        .short_role_info_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "信息";
    }
    if lex
        .short_role_emotion_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "情绪";
    }
    if ctx.ba_regex.find_iter(stripped).any(|m| m.is_ok())
        || lex.verb_hints.iter().any(|v| stripped.contains(v.as_str()))
    {
        return "动作";
    }
    if stripped.matches('，').count() + stripped.matches('、').count() >= 2 {
        return "清单";
    }
    "其他"
}

/// 短句角色汇总（对齐 `summarize_short_roles`：固定角色序、计数降序）。
pub(crate) fn summarize_short_roles(
    ctx: &DraftContext,
    items: &[&crate::text::SentenceInfo],
) -> Vec<ShortRole> {
    let mut counts = Counter::default();
    for item in items {
        counts.add(classify_short_sentence_role(ctx, &item.text));
    }
    let order_of = |role: &str| -> u32 {
        match role {
            "对白" => 0,
            "动作" => 1,
            "信息" => 2,
            "判断" => 3,
            "情绪" => 4,
            "清单" => 5,
            "其他" => 6,
            _ => 99,
        }
    };
    let mut rows: Vec<ShortRole> = counts
        .entries()
        .iter()
        .map(|(role, count)| ShortRole {
            role: role.clone(),
            count: *count,
        })
        .collect();
    rows.sort_by(|a, b| {
        order_of(&a.role)
            .cmp(&order_of(&b.role))
            .then(b.count.cmp(&a.count))
    });
    rows
}

/// 由角色汇总给出短句连发建议（对齐 `suggest_short_run_action`，平手取首现）。
pub(crate) fn suggest_short_run_action(roles: &[ShortRole]) -> String {
    let top_role = roles.iter().fold(None, |best: Option<&ShortRole>, r| {
        if best.is_none_or(|b| r.count > b.count) {
            Some(r)
        } else {
            best
        }
    });
    match top_role.map_or("其他", |r| r.role.as_str()) {
        "对白" => "保留最锋利的一两句，其余用动作、环境声或第三方反应打断。".to_string(),
        "动作" => "保留关键动作，补动作因果、阻力或结果，避免操作日志。".to_string(),
        "信息" => "把信息拆成发现、误读、排除和后果，不要连续报材料。".to_string(),
        "判断" => "人物台词可留；旁白判断优先换成证据、动作或误读。".to_string(),
        "情绪" => "用身体反应、声音和场面反馈承载情绪，不要连续短评。".to_string(),
        "清单" => "保留一个清单节奏，其余并入动作过程或视角变化。".to_string(),
        _ => "先判断这些短句是否都必要；只保留一个节奏点，其余展开。".to_string(),
    }
}

/// `format_short_roles`：`角色 x计数` 用分隔符连接。
pub(crate) fn format_short_roles(roles: &[ShortRole], separator: &str) -> String {
    roles
        .iter()
        .map(|r| format!("{} x{}", r.role, r.count))
        .collect::<Vec<_>>()
        .join(separator)
}

/// 对白轴分类（对齐 `classify_dialogue_axis`）。
fn classify_dialogue_axis(ctx: &DraftContext, sentence: &str) -> &'static str {
    let stripped = lstrip_chars(sentence.trim(), LEADING_PUNCT);
    let lex = ctx.lexicon();
    if lex
        .dialogue_axis_device_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "设备声";
    }
    if lex
        .dialogue_axis_third_party_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "第三方";
    }
    if lex
        .dialogue_axis_env_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "环境";
    }
    if lex
        .dialogue_axis_action_terms
        .iter()
        .any(|t| stripped.contains(t.as_str()))
    {
        return "动作";
    }
    ""
}

/// 对白轴转轴建议（对齐 `suggest_dialogue_axis_action`）。
fn suggest_dialogue_axis_action(axes: &[String]) -> String {
    if axes.is_empty() {
        return "插入动作、环境变化、第三方打断或设备声，让对白改变场面。".into();
    }
    if !axes.iter().any(|a| a == "动作") {
        return "补一个能改变站位或物件状态的动作，不要只让角色继续接话。".into();
    }
    if !axes.iter().any(|a| a == "环境") && !axes.iter().any(|a| a == "设备声") {
        return "补环境声、设备反馈或空间变化，把话题从互答里拨出来。".into();
    }
    "保留已有转轴，再压掉重复问答或合并台词。".into()
}

/// 对白转轴缺口（对齐 `build_dialogue_axis_gaps`）。
pub(crate) fn build_dialogue_axis_gaps(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> Vec<DialogueAxisGap> {
    if sentence_infos.len() < 4 {
        return Vec::new();
    }
    let window_size = 4;
    let run_max = ctx.thresholds().short_sentence_run_max_chars as usize;
    let mut candidates: Vec<DialogueAxisGap> = Vec::new();
    for start in 0..sentence_infos
        .len()
        .saturating_sub(window_size)
        .saturating_add(1)
    {
        let chunk =
            &sentence_infos[start..start.saturating_add(window_size).min(sentence_infos.len())];
        if chunk.len() < window_size {
            continue;
        }
        let dialogue_count = chunk.iter().filter(|i| is_dialogue_like(&i.text)).count();
        if dialogue_count < window_size {
            continue;
        }
        let axes: Vec<String> = chunk
            .iter()
            .filter_map(|i| {
                let a = classify_dialogue_axis(ctx, &i.text);
                (!a.is_empty()).then_some(a.to_string())
            })
            .collect();
        if !axes.is_empty() {
            continue;
        }
        let question_count = chunk
            .iter()
            .filter(|i| i.text.contains('？') || i.text.contains('?'))
            .count();
        let short_count = chunk.iter().filter(|i| i.chars <= run_max).count();
        let mut reasons = vec![
            format!("纯对白 {dialogue_count}/{window_size}"),
            format!("短句 {short_count}/{window_size}"),
        ];
        if question_count > 0 {
            reasons.push(format!("问句 {question_count}/{window_size}"));
        }
        candidates.push(DialogueAxisGap {
            start_index: chunk[0].index,
            end_index: chunk[chunk.len() - 1].index,
            start_line: chunk[0].line_no,
            end_line: chunk[chunk.len() - 1].line_no,
            score: window_size * 2 + short_count + question_count,
            reasons,
            axes: Vec::new(),
            suggestion: suggest_dialogue_axis_action(&[]),
            sample: chunk.iter().map(|i| i.text.clone()).collect(),
            total_candidates: 0,
        });
    }
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.start_index.cmp(&b.start_index))
    });
    let total_candidates = candidates.len();
    let mut kept: Vec<DialogueAxisGap> = Vec::new();
    let mut occupied: HashSet<usize> = HashSet::new();
    for c in candidates {
        if (c.start_index..=c.end_index).any(|i| occupied.contains(&i)) {
            continue;
        }
        for i in c.start_index..=c.end_index {
            occupied.insert(i);
        }
        let mut g = c;
        g.total_candidates = total_candidates;
        kept.push(g);
        if kept.len() >= sample_limit {
            break;
        }
    }
    kept
}

/// 句上下文标签（对齐 `sentence_context_label`）。
fn sentence_context_label(sentence: &str) -> &'static str {
    let raw = sentence.trim();
    if is_dialogue_like(raw) {
        return "dialogue";
    }
    let stripped = lstrip_chars(raw, LEADING_PUNCT);
    if stripped.starts_with("【Pi】") || stripped.starts_with("Pi") {
        "dialogue"
    } else {
        "narration"
    }
}

/// 判断词语境（对齐 `collect_judgement_contexts`；term 去重保持 matched 原序）。
pub(crate) fn collect_judgement_contexts(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> Vec<JudgementContext> {
    struct Bucket {
        label: &'static str,
        count: usize,
        terms: Counter,
        samples: Vec<JudgementSample>,
    }
    let mut buckets: [Option<Bucket>; 2] = [
        Some(Bucket {
            label: "旁白判断",
            count: 0,
            terms: Counter::default(),
            samples: Vec::new(),
        }),
        Some(Bucket {
            label: "对白判断",
            count: 0,
            terms: Counter::default(),
            samples: Vec::new(),
        }),
    ];
    let judgement_terms = &ctx.lexicon().judgement_context_terms;
    for item in sentence_infos {
        let stripped = lstrip_chars(&item.text, LEADING_PUNCT);
        let matched: Vec<&str> = judgement_terms
            .iter()
            .filter(|t| stripped.contains(t.as_str()))
            .map(|t| t.as_str())
            .collect();
        if matched.is_empty() {
            continue;
        }
        let context = sentence_context_label(&item.text);
        let bucket = &mut buckets[if context == "dialogue" { 1 } else { 0 }]
            .as_mut()
            .unwrap();
        bucket.count += 1;
        let mut seen: Vec<&str> = Vec::new();
        for term in &matched {
            if !seen.contains(term) {
                seen.push(term);
                bucket.terms.add(term);
            }
        }
        if bucket.samples.len() < sample_limit {
            bucket.samples.push(JudgementSample {
                index: item.index,
                line_no: item.line_no,
                terms: matched.iter().map(|s| s.to_string()).collect(),
                text: item.text.clone(),
            });
        }
    }
    let mut out: Vec<JudgementContext> = Vec::new();
    for (i, &context) in ["narration", "dialogue"].iter().enumerate() {
        let bucket = &buckets[i].as_ref().unwrap();
        if bucket.count == 0 {
            continue;
        }
        let warn = context == "narration" && bucket.count >= 3;
        let watch = (context == "narration" && bucket.count >= 2)
            || (context == "dialogue" && bucket.count >= 8);
        let top_terms = bucket
            .terms
            .most_common(6)
            .into_iter()
            .map(|(term, count)| TermCount { term, count })
            .collect();
        out.push(JudgementContext {
            context: context.to_string(),
            label: bucket.label.to_string(),
            count: bucket.count,
            warn,
            watch,
            top_terms,
            samples: bucket.samples.clone(),
        });
    }
    out
}

/// `QUOTE_LINE`（`^\s*[“"【].*`）：对白行判定。
fn is_quote_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('\u{201c}') || trimmed.starts_with('"') || trimmed.starts_with('\u{3010}')
}

/// 逐段对白判定（全行对白或单行引号段）。
fn paragraph_is_dialogue(paragraph: &str) -> Option<String> {
    let stripped = paragraph.trim();
    if stripped.is_empty() {
        return None;
    }
    let lines: Vec<&str> = stripped
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let all = lines.iter().all(|l| is_quote_line(l));
    let single = lines.len() == 1 && quote_ratio(lines[0]) > 0.02 && lines[0].contains('\u{201c}');
    if all || single {
        Some(lines[0].to_string())
    } else {
        None
    }
}

/// 连续引用段（对齐 `detect_dialogue_runs`：≥4 段，1-based 段号）。
pub(crate) fn detect_dialogue_runs(text: &str) -> Vec<(usize, usize, Vec<String>)> {
    let mut runs: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut start_idx = 0usize;
    let paragraphs: Vec<String> = text.split("\n\n").map(|s| s.to_string()).collect();
    let flush = |runs: &mut Vec<(usize, usize, Vec<String>)>,
                 current: &mut Vec<String>,
                 start_idx: usize,
                 end_idx: usize| {
        if current.len() >= 4 {
            let sample = current.iter().take(4).cloned().collect();
            runs.push((start_idx + 1, end_idx, sample));
        }
        current.clear();
    };
    for (idx, para) in paragraphs.iter().enumerate() {
        match paragraph_is_dialogue(para) {
            Some(first) => {
                if current.is_empty() {
                    start_idx = idx;
                }
                current.push(first);
            }
            None => {
                flush(&mut runs, &mut current, start_idx, idx);
            }
        }
    }
    flush(&mut runs, &mut current, start_idx, paragraphs.len());
    runs
}

/// 短对白连续段（对齐 `detect_short_dialogue_runs`：≥4 段且平均 ≤15 字）。
pub(crate) fn detect_short_dialogue_runs(text: &str) -> Vec<(usize, usize, f64, Vec<String>)> {
    let mut flagged: Vec<(usize, usize, f64, Vec<String>)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut start_idx = 0usize;
    let paragraphs: Vec<String> = text.split("\n\n").map(|s| s.to_string()).collect();
    let flush = |flagged: &mut Vec<(usize, usize, f64, Vec<String>)>,
                 current: &mut Vec<String>,
                 start_idx: usize,
                 end_idx: usize| {
        if current.len() >= 4 {
            let total: usize = current
                .iter()
                .map(|item| code_len(strip_chars(item, "\u{201c}\u{201d}\"")))
                .sum();
            let avg_len = total as f64 / current.len() as f64;
            if avg_len <= 15.0 {
                let sample = current.iter().take(4).cloned().collect();
                flagged.push((start_idx + 1, end_idx, round2(avg_len), sample));
            }
        }
        current.clear();
    };
    for (idx, para) in paragraphs.iter().enumerate() {
        match paragraph_is_dialogue(para) {
            Some(first) => {
                if current.is_empty() {
                    start_idx = idx;
                }
                current.push(first);
            }
            None => {
                flush(&mut flagged, &mut current, start_idx, idx);
            }
        }
    }
    flush(&mut flagged, &mut current, start_idx, paragraphs.len());
    flagged
}

/// 短问句乒乓（对齐 `detect_question_ping_pong`：≥3 段，问句且 ≤18 字）。
pub(crate) fn detect_question_ping_pong(text: &str) -> Vec<(usize, usize, Vec<String>)> {
    let mut flagged: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut start_idx = 0usize;
    let paragraphs: Vec<String> = text.split("\n\n").map(|s| s.to_string()).collect();
    let flush = |flagged: &mut Vec<(usize, usize, Vec<String>)>,
                 current: &mut Vec<String>,
                 start_idx: usize,
                 end_idx: usize| {
        if current.len() >= 3 {
            let sample = current.iter().take(4).cloned().collect();
            flagged.push((start_idx + 1, end_idx, sample));
        }
        current.clear();
    };
    for (idx, para) in paragraphs.iter().enumerate() {
        let stripped = para.trim();
        if stripped.is_empty() {
            flush(&mut flagged, &mut current, start_idx, idx);
            continue;
        }
        let lines: Vec<&str> = stripped
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() != 1 || !lines[0].contains('\u{201c}') {
            flush(&mut flagged, &mut current, start_idx, idx);
            continue;
        }
        let line = lines[0];
        let is_question = line.contains('？');
        let short_line = code_len(strip_chars(line, "\u{201c}\u{201d}\"")) <= 18;
        if is_question && short_line {
            if current.is_empty() {
                start_idx = idx;
            }
            current.push(line.to_string());
        } else {
            flush(&mut flagged, &mut current, start_idx, idx);
        }
    }
    flush(&mut flagged, &mut current, start_idx, paragraphs.len());
    flagged
}

/// 引号乒乓（对齐 `detect_quote_ping_pong`：≥4 段且平均 ≤22 字）。
pub(crate) fn detect_quote_ping_pong(text: &str) -> Vec<(usize, usize, f64, Vec<String>)> {
    let mut flagged: Vec<(usize, usize, f64, Vec<String>)> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut start_idx = 0usize;
    let paragraphs: Vec<String> = text.split("\n\n").map(|s| s.to_string()).collect();
    let flush = |flagged: &mut Vec<(usize, usize, f64, Vec<String>)>,
                 current: &mut Vec<String>,
                 start_idx: usize,
                 end_idx: usize| {
        if current.len() >= 4 {
            let total: usize = current
                .iter()
                .map(|item| code_len(strip_chars(item, "\u{201c}\u{201d}\"")))
                .sum();
            let avg_len = total as f64 / current.len() as f64;
            if avg_len <= 22.0 {
                let sample = current.iter().take(6).cloned().collect();
                flagged.push((start_idx + 1, end_idx, round2(avg_len), sample));
            }
        }
        current.clear();
    };
    for (idx, para) in paragraphs.iter().enumerate() {
        let stripped = para.trim();
        if stripped.is_empty() {
            flush(&mut flagged, &mut current, start_idx, idx);
            continue;
        }
        let lines: Vec<&str> = stripped
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() != 1 || !lines[0].contains('\u{201c}') {
            flush(&mut flagged, &mut current, start_idx, idx);
            continue;
        }
        if current.is_empty() {
            start_idx = idx;
        }
        current.push(lines[0].to_string());
    }
    flush(&mut flagged, &mut current, start_idx, paragraphs.len());
    flagged
}

/// A/B 交替说话人（对齐 `detect_a_b_turns`：4 连 `A->B->A->B`，1-based 段号）。
pub(crate) fn detect_a_b_turns(ctx: &DraftContext, text: &str) -> Vec<AbTurn> {
    let paragraphs: Vec<String> = text.split("\n\n").map(|s| s.to_string()).collect();
    let mut speakers: Vec<(usize, String)> = Vec::new();
    for (i, para) in paragraphs.iter().enumerate() {
        let stripped = para.trim();
        if stripped.is_empty() {
            continue;
        }
        let mut found: Option<String> = None;
        for pattern in &ctx.speaker_patterns {
            if let Ok(Some(caps)) = pattern.captures(stripped) {
                let mut expanded = String::new();
                caps.expand("$1", &mut expanded);
                found = Some(expanded);
                break;
            }
        }
        if let Some(speaker) = found {
            if !speaker.is_empty() {
                speakers.push((i + 1, speaker));
            }
        }
    }
    let mut flagged: Vec<AbTurn> = Vec::new();
    for idx in 0..speakers.len().saturating_sub(3) {
        let seq = &speakers[idx..idx + 4];
        let names: Vec<&str> = seq.iter().map(|(_, s)| s.as_str()).collect();
        if names[0] == names[2] && names[1] == names[3] && names[0] != names[1] {
            flagged.push(AbTurn {
                paragraph: seq[0].0,
                pattern: names.join(" -> "),
            });
        }
    }
    flagged
}

/// 情绪规则标签（对齐 `classify_dialogue_emotion`：首条命中规则优先）。
fn classify_dialogue_emotion<'a>(ctx: &'a DraftContext, text: &str) -> &'a str {
    let lex = ctx.lexicon();
    for (label, terms) in &lex.dialogue_emotion_rules.rules {
        if terms.iter().any(|term| text.contains(term)) {
            return label.as_str();
        }
    }
    if text.contains('？') || text.contains('?') {
        return "pressure";
    }
    if text.contains('\u{ff01}') {
        return "hostility";
    }
    "neutral"
}

/// 对白情绪曲线（对齐 `build_dialogue_emotion_profile`）。
pub fn build_dialogue_emotion_profile(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> DialogueEmotions {
    let dialogue_items: Vec<&crate::text::SentenceInfo> = sentence_infos
        .iter()
        .filter(|item| is_dialogue_like(&item.text))
        .collect();
    let mut emotion_counter = Counter::default();
    let mut samples: Vec<EmotionSample> = Vec::new();
    let mut last_label = String::new();
    let mut shift_count = 0usize;
    for item in &dialogue_items {
        let text = item.text.trim();
        let lex = ctx.lexicon();
        let mut labels: Vec<String> = lex
            .dialogue_emotion_rules
            .rules
            .iter()
            .filter(|(_, terms)| terms.iter().any(|term| text.contains(term)))
            .map(|(label, _)| label.clone())
            .collect();
        if labels.is_empty() {
            if text.contains('？') || text.contains('?') {
                labels = vec!["pressure".to_string()];
            } else if text.contains('\u{ff01}') {
                labels = vec!["hostility".to_string()];
            }
        }
        let primary = labels.first().map_or("neutral", |s| s.as_str());
        emotion_counter.add(primary);
        if !samples.is_empty()
            && primary != "neutral"
            && !last_label.is_empty()
            && primary != last_label
        {
            shift_count += 1;
        }
        if primary != "neutral" {
            last_label = primary.to_string();
        }
        if samples.len() < sample_limit {
            samples.push(EmotionSample {
                line_no: item.line_no,
                label: primary.to_string(),
                labels: if labels.is_empty() {
                    vec!["neutral".to_string()]
                } else {
                    labels.clone()
                },
                text: text.to_string(),
            });
        }
    }
    let (dominant_label, dominant_count) = emotion_counter
        .most_common(1)
        .into_iter()
        .next()
        .unwrap_or_else(|| ("neutral".to_string(), 0));
    let non_neutral: usize = emotion_counter
        .entries()
        .iter()
        .filter(|(label, _)| label.as_str() != "neutral")
        .map(|(_, count)| *count)
        .sum();
    let flatness_warn = dialogue_items.len() >= 6
        && dominant_label != "neutral"
        && dominant_count as f64 / dialogue_items.len().max(1) as f64 >= 0.7;
    let volatility_warn = shift_count >= 4 && non_neutral >= 5;
    DialogueEmotions {
        dialogue_sentences: dialogue_items.len(),
        emotion_counts: CountMap::new(emotion_counter.entries().to_vec()),
        dominant_emotion: dominant_label,
        dominant_ratio: if dialogue_items.is_empty() {
            0.0
        } else {
            round4f(dominant_count as f64 / dialogue_items.len().max(1) as f64)
        },
        shift_count,
        flatness_warn,
        volatility_warn,
        samples,
    }
}

/// 从对白句提取说话人名（对齐 `extract_speaker_name`）。
fn extract_speaker_name(ctx: &DraftContext, text: &str) -> String {
    let cleaned = text.trim();
    let lex = ctx.lexicon();
    let suffixes = &ctx.rules.draft.speaker.suffixes;
    for pattern in &ctx.speaker_line_patterns {
        let Ok(Some(caps)) = pattern.captures(cleaned) else {
            continue;
        };
        let mut raw = String::new();
        caps.expand("$1", &mut raw);
        let mut name = strip_chars(&raw, LEADING_PUNCT).to_string();
        for sfx in suffixes {
            if name.ends_with(sfx.as_str()) && code_len(&name) > code_len(sfx) {
                name = prefix_chars(&name, code_len(&name) - code_len(sfx))
                    .trim()
                    .to_string();
                break;
            }
        }
        let first = name.chars().next();
        let rest: String = name.chars().skip(1).collect();
        if code_len(&name) >= 2
            && matches!(
                first,
                Some('\u{4ed6}') | Some('\u{5979}') | Some('\u{6211}') | Some('\u{4f60}')
            )
            && suffixes.iter().any(|s| s == &rest)
        {
            name = first.unwrap().to_string();
        }
        if !name.is_empty()
            && !lex.character_name_stoplist.iter().any(|s| s == &name)
            && code_len(&name) <= 12
        {
            return name;
        }
    }
    String::new()
}

/// 角色对白画像（对齐 `build_character_voice_profile`：
/// 每角色行级统计 + 同质化对检测）。
pub fn build_character_voice_profile(
    ctx: &DraftContext,
    sentence_infos: &[crate::text::SentenceInfo],
    sample_limit: usize,
) -> CharacterVoice {
    #[derive(Debug)]
    struct SpeakerStats {
        lines: usize,
        chars: usize,
        questions: usize,
        exclaims: usize,
        judgements: usize,
        short_lines: usize,
        emotion_counter: Counter,
        samples: Vec<SpeakerSample>,
    }
    let mut speaker_counter = Counter::default();
    let mut speaker_stats: Vec<(String, SpeakerStats)> = Vec::new();
    let mut unknown_count = 0usize;
    let lex = ctx.lexicon();
    let run_max = ctx.thresholds().short_sentence_run_max_chars as usize;
    for item in sentence_infos {
        if !is_dialogue_like(&item.text) {
            continue;
        }
        let text = item.text.trim();
        let speaker = extract_speaker_name(ctx, text);
        if speaker.is_empty() {
            unknown_count += 1;
            continue;
        }
        speaker_counter.add(&speaker);
        let idx = match speaker_stats.iter().position(|(s, _)| *s == speaker) {
            Some(idx) => idx,
            None => {
                speaker_stats.push((
                    speaker.clone(),
                    SpeakerStats {
                        lines: 0,
                        chars: 0,
                        questions: 0,
                        exclaims: 0,
                        judgements: 0,
                        short_lines: 0,
                        emotion_counter: Counter::default(),
                        samples: Vec::new(),
                    },
                ));
                speaker_stats.len() - 1
            }
        };
        let entry = &mut speaker_stats[idx].1;
        entry.lines += 1;
        let pcc = prose_char_count(text);
        entry.chars += pcc;
        entry.questions += usize::from(text.contains('？') || text.contains('?'));
        entry.exclaims += usize::from(text.contains('\u{ff01}'));
        entry.judgements += usize::from(
            lex.judgement_context_terms
                .iter()
                .any(|term| text.contains(term.as_str())),
        );
        entry.short_lines += usize::from(pcc <= run_max);
        let emotion = classify_dialogue_emotion(ctx, text);
        entry.emotion_counter.add(emotion);
        if entry.samples.len() < sample_limit {
            entry.samples.push(SpeakerSample {
                line_no: item.line_no,
                text: text.to_string(),
                emotion: emotion.to_string(),
            });
        }
    }
    let mut speakers: Vec<SpeakerProfile> = Vec::new();
    for (speaker, _count) in speaker_counter.most_common(sample_limit * 2) {
        let stats = speaker_stats
            .iter()
            .find(|(s, _)| *s == speaker)
            .map(|(_, st)| st)
            .unwrap();
        let lines = stats.lines.max(1);
        let (dominant_emotion, dominant_count) = stats
            .emotion_counter
            .most_common(1)
            .into_iter()
            .next()
            .unwrap_or_else(|| ("neutral".to_string(), 0));
        speakers.push(SpeakerProfile {
            speaker,
            lines: stats.lines,
            avg_chars: round2(stats.chars as f64 / lines as f64),
            question_ratio: round4f(stats.questions as f64 / lines as f64),
            exclaim_ratio: round4f(stats.exclaims as f64 / lines as f64),
            judgement_ratio: round4f(stats.judgements as f64 / lines as f64),
            short_ratio: round4f(stats.short_lines as f64 / lines as f64),
            dominant_emotion,
            dominant_ratio: round4f(dominant_count as f64 / lines as f64),
            samples: stats.samples.iter().take(sample_limit).cloned().collect(),
        });
    }
    let identifiable_lines: usize = speakers.iter().map(|s| s.lines).sum();
    let coverage_ratio =
        round4f(identifiable_lines as f64 / (identifiable_lines + unknown_count).max(1) as f64);
    let comparable: Vec<&SpeakerProfile> = speakers.iter().filter(|s| s.lines >= 3).collect();
    let mut homogenized_pairs: Vec<String> = Vec::new();
    for (i, left) in comparable.iter().enumerate() {
        for right in &comparable[i + 1..] {
            if left.dominant_emotion == right.dominant_emotion
                && (left.avg_chars - right.avg_chars).abs() <= 3.0
                && (left.question_ratio - right.question_ratio).abs() <= 0.2
                && (left.short_ratio - right.short_ratio).abs() <= 0.2
            {
                homogenized_pairs.push(format!(
                    "{}~{} emotion={} avg={}/{}",
                    left.speaker,
                    right.speaker,
                    left.dominant_emotion,
                    float_repr(left.avg_chars),
                    float_repr(right.avg_chars),
                ));
            }
        }
    }
    let warn = !homogenized_pairs.is_empty() && comparable.len() >= 2;
    let dominant_speaker = speakers
        .first()
        .map(|s| s.speaker.clone())
        .unwrap_or_default();
    let dominant_ratio = if speakers.is_empty() {
        0.0
    } else {
        round4f(speakers[0].lines as f64 / identifiable_lines.max(1) as f64)
    };
    CharacterVoice {
        speaker_count: speakers.len(),
        identified_lines: identifiable_lines,
        unknown_lines: unknown_count,
        coverage_ratio,
        dominant_speaker,
        dominant_ratio,
        warn,
        homogenized_pairs: homogenized_pairs.into_iter().take(sample_limit).collect(),
        speakers: speakers.into_iter().take(sample_limit).collect(),
    }
}
