//! `reports-profiles` 子命令：
//! 面向研究的章节句子画像（profiles/*.md + 每 story 目录 SUMMARY.md 镜像树）。
//!
//! - 章节分析复用 `audit::draft::analyze_path`（语料学习缺省开启，
//!   `build_corpus_profile(ctx.corpus_paths_for_targets(files))` 契约同 `run`）；
//! - 路径镜像复用 `stats::draft::stats_path_for`（`--output-root` 分支
//!   逐行稳定输出；`profile_path_for` 的 `drafts` 定位与 `relative_to` 语义）；
//! - 中文文案固定；浮点走 `audit::draft::float_repr`（浮点展示语义），
//!   bool 走 `True`/`False`。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::{
    analyze_path, build_corpus_profile, float_repr, AaBbPattern, Analysis, BattleSequence,
    DraftContext, EmotionSample, FatigueWindow, LearnedFilterMetric, OverlapEntry, PhraseCount,
    SceneBlock, SpeakerProfile, TermCount, TrackedTermWindow,
};
use crate::config;
use crate::input::write_text;
use crate::rules::{build_template_bank, CustomTemplateMetric, TrackedMetric};
use crate::stats::draft::{chapter_sort_key, collect_chapter_files, stats_path_for, Ctr};

/// `reports-profiles` 子命令参数（位置 `paths` nargs+、
/// `--sample-limit` 缺省 8、`--output-root` 可选镜像根）。
#[derive(Debug, Clone)]
pub struct ProfileOptions {
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// 位置参数：草稿章文件或目录。
    pub paths: Vec<PathBuf>,
    /// 每节最多渲染的条数（默认 8）。
    pub sample_limit: usize,
    /// 可选镜像根目录；缺省写小说本地 draft-stats 树。
    pub output_root: Option<PathBuf>,
}

/// bool 渲染（`True`/`False`）。
fn bool_str(flag: bool) -> &'static str {
    if flag {
        "True"
    } else {
        "False"
    }
}

/// `str(x) or 'none'`（空串 → none）。
fn none_or(value: &str) -> &str {
    if value.is_empty() {
        "none"
    } else {
        value
    }
}

/// `profile_path_for`：缺省 `stats_path_for(draft).parent / profiles / {stem}.md`；
/// 有 `output_root` 时镜像到 `output_root / {novel} / {draft-stats 相对树} / profiles / {stem}.md`。
pub fn profile_path_for(
    draft_path: &Path,
    output_root: Option<&Path>,
    paths: &crate::config::PathsConfig,
) -> Result<PathBuf> {
    let stats = stats_path_for(draft_path, None, paths)?;
    let stats_dir = stats.parent().context("stats 路径无父目录")?;
    let stem = draft_path
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("章节文件名必须为合法 UTF-8: {}", draft_path.display()))?;
    let out_dir = match output_root {
        None => stats_dir.join("profiles"),
        Some(root) => {
            let comps: Vec<std::path::Component> = draft_path.components().collect();
            let idx = comps
                .iter()
                .position(|c| c.as_os_str() == "drafts")
                .with_context(|| {
                    format!("Path does not live under drafts/: {}", draft_path.display())
                })?;
            let novel_root: PathBuf = comps[..idx].iter().collect();
            let relative = stats_dir
                .strip_prefix(&novel_root)
                .with_context(|| "stats 目录无法相对化到 novel 根")?;
            let novel_name = novel_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            root.join(novel_name).join(relative).join("profiles")
        }
    };
    Ok(out_dir.join(format!("{stem}.md")))
}

/// 排序项（`phrase`/`term` + `count` 的通用行）。
struct RankedItem {
    label: String,
    count: usize,
}

/// 指标行（`name`/`count`/`per_10k`/`warn`/`note`；`per_10k` 缺键 → `None`）。
struct MetricItem {
    name: String,
    count: usize,
    per_10k: Option<f64>,
    warn: bool,
    note: String,
}

/// 热窗口行（fatigue / tracked-term 窗口的通用行）。
struct HotWindow {
    start_index: usize,
    end_index: usize,
    start_line: usize,
    end_line: usize,
    score: usize,
    reasons: Vec<String>,
    sample: Vec<String>,
}

/// `format_ranked_items`：`- `{label}` x{count}`（空 → `- 无`）。
fn format_ranked_items(items: &[RankedItem], sample_limit: usize) -> Vec<String> {
    if items.is_empty() {
        return vec!["- 无".to_string()];
    }
    items
        .iter()
        .take(sample_limit)
        .map(|item| format!("- `{}` x{}", item.label, item.count))
        .collect()
}

/// `format_metric_items`：
/// `- `{name}` x{count}` [` per_10k=`..``] [` `WARN`] [：note]（空 → `- 无`）。
fn format_metric_items(items: &[MetricItem], sample_limit: usize) -> Vec<String> {
    if items.is_empty() {
        return vec!["- 无".to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    for item in items.iter().take(sample_limit) {
        let mut line = format!("- `{}` x{}", item.name, item.count);
        if let Some(per) = item.per_10k {
            line.push_str(&format!(" per_10k=`{}`", float_repr(per)));
        }
        if item.warn {
            line.push_str(" `WARN`");
        }
        let note = item.note.trim();
        if !note.is_empty() {
            line.push_str(&format!("：{note}"));
        }
        lines.push(line);
    }
    lines
}

/// `format_hot_windows`（`title_key` 固定取 `reasons`）。
fn format_hot_windows(items: &[HotWindow], sample_limit: usize) -> Vec<String> {
    if items.is_empty() {
        return vec!["- 无".to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    for item in items.iter().take(sample_limit) {
        let reasons = item.reasons.join("、");
        let reasons = if reasons.is_empty() {
            "局部高压".to_string()
        } else {
            reasons
        };
        let sample = item
            .sample
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!(
            "- `S{}-{}` `L{}-{}` score=`{}`：{reasons}",
            item.start_index, item.end_index, item.start_line, item.end_line, item.score
        ));
        if !sample.is_empty() {
            lines.push(format!("  样例：{sample}"));
        }
    }
    lines
}

/// 段间分隔：标题 + 内容行 + 空行。
fn push_section(lines: &mut Vec<String>, title: &str, block: &[String]) {
    lines.push(format!("## {title}"));
    lines.extend(block.iter().cloned());
    lines.push(String::new());
}

/// `build_profile_report`：单章句子画像 markdown（逐字渲染）。
pub fn build_profile_report(draft_path: &Path, analysis: &Analysis, sample_limit: usize) -> String {
    let summary = &analysis.summary;
    let dialogue = &analysis.dialogue;
    let profile = &analysis.corpus_profile;
    let stem = draft_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    let mut lines: Vec<String> = vec![format!("# {stem} Sentence Profile"), String::new()];
    lines.push(format!("- source: `{}`", draft_path.display()));
    lines.push(format!("- chars: `{}`", summary.chars));
    lines.push(format!("- sentences: `{}`", summary.sentences));
    lines.push(format!("- paragraphs: `{}`", summary.paragraphs));
    lines.push(format!(
        "- avg_sentence_chars: `{}`",
        float_repr(summary.avg_sentence_chars)
    ));
    lines.push(format!(
        "- short_ratio: `{}`",
        float_repr(summary.short_sentence_ratio)
    ));
    lines.push(format!(
        "- quote_ratio: `{}`",
        float_repr(summary.quote_ratio)
    ));
    lines.push(String::new());

    lines.push("## Chapter Fingerprint".to_string());
    let top_fatigue = analysis
        .style_fatigue
        .iter()
        .filter(|item| item.status == "WARN")
        .map(|item| format!("{} x{}", item.family, item.count))
        .collect::<Vec<_>>()
        .join(", ");
    let top_fatigue = if top_fatigue.is_empty() {
        "无".to_string()
    } else {
        top_fatigue
    };
    let top_templates = analysis
        .template_candidates
        .iter()
        .take(5)
        .map(|item| format!("{} x{}", item.name, item.count))
        .collect::<Vec<_>>()
        .join(", ");
    let top_templates = if top_templates.is_empty() {
        "无".to_string()
    } else {
        top_templates
    };
    lines.push(format!("- fatigue: {top_fatigue}"));
    lines.push(format!("- template_candidates: {top_templates}"));
    lines.push(format!(
        "- dialogue: axis_gaps=`{}` short_quote_runs=`{}` ping_pong=`{}`",
        dialogue.dialogue_axis_gaps.len(),
        dialogue.short_quote_runs.len(),
        dialogue.quote_ping_pong.len()
    ));
    lines.push(format!(
        "- windows: fatigue=`{}` tracked_terms=`{}`",
        analysis.fatigue_window_count, analysis.tracked_term_window_count
    ));
    lines.push(format!(
        "- narrative: scene_blocks=`{}` tone=`{}` battle_sequences=`{}` viewpoint_anchor=`{}`",
        analysis.scene_map.block_count,
        analysis.tone_profile.dominant_tone,
        analysis.battle_profile.sequence_count,
        none_or(&analysis.viewpoint_profile.dominant_anchor)
    ));
    lines.push(format!(
        "- character_voice: speakers=`{}` dominant=`{}` coverage=`{}` warn=`{}`",
        analysis.character_voice.speaker_count,
        none_or(&analysis.character_voice.dominant_speaker),
        float_repr(analysis.character_voice.coverage_ratio),
        bool_str(analysis.character_voice.warn)
    ));
    lines.push(String::new());

    let ranked = |items: &[PhraseCount]| {
        items
            .iter()
            .map(|i| RankedItem {
                label: i.phrase.clone(),
                count: i.count,
            })
            .collect::<Vec<_>>()
    };
    let term_ranked = |items: &[TermCount]| {
        items
            .iter()
            .map(|i| RankedItem {
                label: i.term.clone(),
                count: i.count,
            })
            .collect::<Vec<_>>()
    };
    let metric_rows = |items: &[TrackedMetric]| {
        items
            .iter()
            .map(|i| MetricItem {
                name: i.name.clone(),
                count: i.count,
                per_10k: Some(i.per_10k),
                warn: i.warn,
                note: i.note.clone(),
            })
            .collect::<Vec<_>>()
    };
    let template_metric_rows = |items: &[CustomTemplateMetric]| {
        items
            .iter()
            .map(|i| MetricItem {
                name: i.name.clone(),
                count: i.count,
                per_10k: Some(i.per_10k),
                warn: i.warn,
                note: i.note.clone(),
            })
            .collect::<Vec<_>>()
    };
    let filter_metric_rows = |items: &[LearnedFilterMetric]| {
        items
            .iter()
            .map(|i| MetricItem {
                name: i.name.clone(),
                count: i.count,
                per_10k: Some(i.per_10k),
                warn: i.warn,
                note: i.note.clone(),
            })
            .collect::<Vec<_>>()
    };
    let aa_bb_metric_rows = |items: &[AaBbPattern]| {
        items
            .iter()
            .map(|i| MetricItem {
                name: i.name.clone(),
                count: i.count,
                per_10k: None,
                warn: i.warn,
                note: i.note.clone(),
            })
            .collect::<Vec<_>>()
    };
    let fatigue_hot = |items: &[FatigueWindow]| {
        items
            .iter()
            .map(|i| HotWindow {
                start_index: i.start_index,
                end_index: i.end_index,
                start_line: i.start_line,
                end_line: i.end_line,
                score: i.score,
                reasons: i.reasons.clone(),
                sample: i.sample.clone(),
            })
            .collect::<Vec<_>>()
    };
    let tracked_hot = |items: &[TrackedTermWindow]| {
        items
            .iter()
            .map(|i| HotWindow {
                start_index: i.start_index,
                end_index: i.end_index,
                start_line: i.start_line,
                end_line: i.end_line,
                score: i.score,
                reasons: i.reasons.clone(),
                sample: i.sample.clone(),
            })
            .collect::<Vec<_>>()
    };

    let skeleton_rows = ranked(&analysis.sentence_patterns);
    let start_rows = ranked(&analysis.sentence_starts);
    let subject_rows = ranked(&analysis.subject_leads);
    let paragraph_rows = ranked(&analysis.paragraph_leads);
    let clause_rows = ranked(&analysis.clause_prefixes);
    let parallel_rows = ranked(&analysis.parallel_clauses);
    let judgement_rows = ranked(&analysis.judgement_endings);
    let short_phrase_rows = term_ranked(&analysis.short_phrases);
    let term_rows = term_ranked(&analysis.terms);
    let tracked_rows = metric_rows(&analysis.tracked_terms);
    let template_rows = template_metric_rows(&analysis.custom_templates);
    let filter_rows = filter_metric_rows(&analysis.learned_filters);
    let aa_bb_rows = aa_bb_metric_rows(&analysis.aa_bb_patterns);
    let fatigue_window_rows = fatigue_hot(&analysis.fatigue_windows);
    let tracked_window_rows = tracked_hot(&analysis.tracked_term_windows);

    let sections: Vec<(&str, Vec<String>)> = vec![
        (
            "Sentence Skeletons",
            format_ranked_items(&skeleton_rows, sample_limit),
        ),
        (
            "Sentence Starts",
            format_ranked_items(&start_rows, sample_limit),
        ),
        (
            "Subject Leads",
            format_ranked_items(&subject_rows, sample_limit),
        ),
        (
            "Paragraph Leads",
            format_ranked_items(&paragraph_rows, sample_limit),
        ),
        (
            "Clause Prefixes",
            format_ranked_items(&clause_rows, sample_limit),
        ),
        (
            "Parallel Clauses",
            format_ranked_items(&parallel_rows, sample_limit),
        ),
        (
            "Judgement Endings",
            format_ranked_items(&judgement_rows, sample_limit),
        ),
        (
            "Structural Phrases",
            format_ranked_items(&short_phrase_rows, sample_limit),
        ),
        (
            "Frequent Fragments",
            format_ranked_items(&term_rows, sample_limit),
        ),
        (
            "Tracked Terms",
            format_metric_items(&tracked_rows, sample_limit),
        ),
        (
            "Template Bank Hits",
            format_metric_items(&template_rows, sample_limit),
        ),
        (
            "Learned Filters",
            format_metric_items(&filter_rows, sample_limit),
        ),
        (
            "AA/BB Patterns",
            format_metric_items(&aa_bb_rows, sample_limit),
        ),
        (
            "Hot Fatigue Windows",
            format_hot_windows(&fatigue_window_rows, sample_limit),
        ),
        (
            "Tracked Term Windows",
            format_hot_windows(&tracked_window_rows, sample_limit),
        ),
    ];
    for (title, block) in sections {
        push_section(&mut lines, title, &block);
    }

    lines.push("## Narrative Signals".to_string());
    let scene_map = &analysis.scene_map;
    let role_summary = scene_map
        .role_counts
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join("，");
    let role_summary = if role_summary.is_empty() {
        "无".to_string()
    } else {
        role_summary
    };
    lines.push(format!(
        "- scene_blocks=`{}` dominant_role=`{}` dominance_ratio=`{}` switches=`{}` warn=`{}`",
        scene_map.block_count,
        scene_map.dominant_role,
        float_repr(scene_map.dominance_ratio),
        scene_map.switch_count,
        bool_str(scene_map.warn)
    ));
    lines.push(format!("- scene_roles: {role_summary}"));
    let blocks: Vec<&SceneBlock> = scene_map.blocks.iter().collect();
    for item in blocks.iter().take(sample_limit) {
        lines.push(format!(
            "- block `P{}-{}` `L{}-{}` role=`{}` chars=`{}`",
            item.start_paragraph,
            item.end_paragraph,
            item.start_line,
            item.end_line,
            item.role,
            item.chars
        ));
    }

    let dialogue_emotions = &analysis.dialogue_emotions;
    let emotion_summary = dialogue_emotions
        .emotion_counts
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join("，");
    let emotion_summary = if emotion_summary.is_empty() {
        "无".to_string()
    } else {
        emotion_summary
    };
    lines.push(format!(
        "- dialogue_emotion dominant=`{}` ratio=`{}` shifts=`{}` flat_warn=`{}` volatility_warn=`{}`",
        dialogue_emotions.dominant_emotion,
        float_repr(dialogue_emotions.dominant_ratio),
        dialogue_emotions.shift_count,
        bool_str(dialogue_emotions.flatness_warn),
        bool_str(dialogue_emotions.volatility_warn)
    ));
    lines.push(format!("- emotion_counts: {emotion_summary}"));
    let emotion_samples: Vec<&EmotionSample> = dialogue_emotions.samples.iter().collect();
    for item in emotion_samples.iter().take(sample_limit) {
        lines.push(format!(
            "- `L{}` `{}` {}",
            item.line_no, item.label, item.text
        ));
    }

    let character_voice = &analysis.character_voice;
    lines.push(format!(
        "- character_voice dominant=`{}` speakers=`{}` identified=`{}` unknown=`{}` coverage=`{}` warn=`{}`",
        none_or(&character_voice.dominant_speaker),
        character_voice.speaker_count,
        character_voice.identified_lines,
        character_voice.unknown_lines,
        float_repr(character_voice.coverage_ratio),
        bool_str(character_voice.warn)
    ));
    let speakers: Vec<&SpeakerProfile> = character_voice.speakers.iter().collect();
    for item in speakers.iter().take(sample_limit) {
        lines.push(format!(
            "- speaker `{}` lines=`{}` avg=`{}` q=`{}` short=`{}` judgement=`{}` emotion=`{}`",
            item.speaker,
            item.lines,
            float_repr(item.avg_chars),
            float_repr(item.question_ratio),
            float_repr(item.short_ratio),
            float_repr(item.judgement_ratio),
            item.dominant_emotion
        ));
    }
    let pairs: Vec<&String> = character_voice.homogenized_pairs.iter().collect();
    for item in pairs.iter().take(sample_limit) {
        lines.push(format!("- homogenized `{item}`"));
    }

    let tone_profile = &analysis.tone_profile;
    let tone_summary = tone_profile
        .tone_counts
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join("，");
    let tone_summary = if tone_summary.is_empty() {
        "无".to_string()
    } else {
        tone_summary
    };
    lines.push(format!(
        "- tone dominant=`{}` stable_ratio=`{}` switches=`{}` warn=`{}`",
        tone_profile.dominant_tone,
        float_repr(tone_profile.stable_ratio),
        tone_profile.switch_count,
        bool_str(tone_profile.warn)
    ));
    lines.push(format!("- tone_counts: {tone_summary}"));

    let battle_profile = &analysis.battle_profile;
    lines.push(format!(
        "- battle sequences=`{}` max_run=`{}` action=`{}` result=`{}` damage=`{}` ratio=`{}` warn=`{}`",
        battle_profile.sequence_count,
        battle_profile.max_sequence_sentences,
        battle_profile.action_hits,
        battle_profile.result_hits,
        battle_profile.damage_hits,
        float_repr(battle_profile.result_ratio),
        bool_str(battle_profile.warn)
    ));
    let battle_samples: Vec<&BattleSequence> = battle_profile.samples.iter().collect();
    for item in battle_samples.iter().take(sample_limit) {
        lines.push(format!(
            "- battle `S{}-{}` `L{}-{}` action=`{}` result=`{}` damage=`{}`",
            item.start_index,
            item.end_index,
            item.start_line,
            item.end_line,
            item.action_hits,
            item.result_hits,
            item.damage_hits
        ));
    }

    let viewpoint_profile = &analysis.viewpoint_profile;
    let anchor_summary = viewpoint_profile
        .anchor_counts
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join("，");
    let anchor_summary = if anchor_summary.is_empty() {
        "无".to_string()
    } else {
        anchor_summary
    };
    lines.push(format!(
        "- viewpoint dominant=`{}` switches=`{}` overlaps=`{}` warn=`{}`",
        none_or(&viewpoint_profile.dominant_anchor),
        viewpoint_profile.switch_count,
        viewpoint_profile.overlap_count,
        bool_str(viewpoint_profile.warn)
    ));
    lines.push(format!("- viewpoint_anchors: {anchor_summary}"));
    let overlaps: Vec<&OverlapEntry> = viewpoint_profile.overlaps.iter().collect();
    for item in overlaps.iter().take(sample_limit) {
        lines.push(format!(
            "- `L{}` `{}` {}",
            item.line_no,
            item.anchors.join(","),
            item.text
        ));
    }
    lines.push(String::new());

    lines.push("## Style Fatigue".to_string());
    if !analysis.style_fatigue.is_empty() {
        for item in &analysis.style_fatigue {
            let evidence = item
                .evidence
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("；");
            let evidence = if evidence.is_empty() {
                "无".to_string()
            } else {
                evidence
            };
            lines.push(format!(
                "- `{}` `{}` x{}：{} 建议：{} 证据：{evidence}",
                item.status, item.family, item.count, item.risk, item.reduce
            ));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Review Reminders".to_string());
    if !analysis.review_reminders.is_empty() {
        for item in analysis.review_reminders.iter().take(sample_limit) {
            lines.push(format!(
                "- `{}` `{}` {}：{} 动作：{}",
                item.priority, item.category, item.title, item.reason, item.action
            ));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Corpus Baseline".to_string());
    if profile.enabled {
        lines.push(format!(
            "- sources=`{}` chars=`{}` draft_chars=`{}`",
            profile.source_count, profile.chars, profile.draft_chars
        ));
        if let crate::audit::draft::BaselineJson::Values(baseline) =
            &profile.sentence_length_baseline
        {
            lines.push(format!(
                "- sentence_length: p10=`{}` p25=`{}` median=`{}` avg=`{}` short_ratio=`{}`",
                baseline.p10_chars,
                baseline.p25_chars,
                baseline.median_chars,
                float_repr(baseline.avg_chars),
                float_repr(baseline.short_ratio)
            ));
        }
        let learned_leads = profile
            .learned_sentence_leads
            .iter()
            .take(sample_limit)
            .map(|item| format!("{} x{}", item.phrase, item.count))
            .collect::<Vec<_>>()
            .join(", ");
        let learned_leads = if learned_leads.is_empty() {
            "无".to_string()
        } else {
            learned_leads
        };
        lines.push(format!("- learned_sentence_leads: {learned_leads}"));
    } else {
        lines.push("- 未启用".to_string());
    }
    lines.push(String::new());

    lines.join("\n") + "\n"
}

/// `build_story_summary`：story 级 SUMMARY.md（逐字渲染；内部按
/// `chapter_sort_key` 排序）。
pub fn build_story_summary(
    story_dir: &Path,
    analyses: &[(PathBuf, &Analysis)],
    sample_limit: usize,
) -> String {
    let mut ordered: Vec<(PathBuf, &Analysis)> = analyses.to_vec();
    ordered.sort_by_key(|a| chapter_sort_key(&a.0));

    let mut counter_map: Vec<(&str, Ctr)> = vec![
        ("sentence_patterns", Ctr::default()),
        ("sentence_starts", Ctr::default()),
        ("judgement_endings", Ctr::default()),
        ("short_phrases", Ctr::default()),
        ("terms", Ctr::default()),
    ];
    let mut fatigue_counter = Ctr::default();
    let mut scene_role_counter = Ctr::default();
    let mut tone_counter = Ctr::default();
    let mut dialogue_emotion_counter = Ctr::default();
    let mut speaker_counter = Ctr::default();
    let mut speaker_warn_counter = Ctr::default();

    let mut lines: Vec<String> = vec!["# Sentence Profile Summary".to_string(), String::new()];
    lines.push(format!("- story: `{}`", story_dir.display()));
    lines.push(format!("- chapters: `{}`", ordered.len()));
    lines.push(String::new());

    lines.push("## Chapters".to_string());
    for (path, analysis) in &ordered {
        let top_pattern = if analysis.sentence_patterns.is_empty() {
            "无".to_string()
        } else {
            analysis.sentence_patterns[0].phrase.clone()
        };
        let top_fragment = if analysis.terms.is_empty() {
            "无".to_string()
        } else {
            analysis.terms[0].term.clone()
        };
        lines.push(format!(
            "- `{}` warn_sections=`{}` top_pattern=`{top_pattern}` top_fragment=`{top_fragment}`",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            analysis.summary.warn_sections
        ));
        scene_role_counter.add(&analysis.scene_map.dominant_role, 1);
        let dominant_tone = &analysis.tone_profile.dominant_tone;
        if !dominant_tone.is_empty() && dominant_tone != "none" {
            tone_counter.add(dominant_tone, 1);
        }
        let dominant_emotion = &analysis.dialogue_emotions.dominant_emotion;
        if !dominant_emotion.is_empty() && dominant_emotion != "neutral" {
            dialogue_emotion_counter.add(dominant_emotion, 1);
        }
        let speakers: Vec<&SpeakerProfile> = analysis.character_voice.speakers.iter().collect();
        for item in speakers.iter().take(sample_limit) {
            speaker_counter.add(&item.speaker, item.lines);
        }
        if analysis.character_voice.warn {
            let pairs: Vec<&String> = analysis.character_voice.homogenized_pairs.iter().collect();
            for item in pairs.iter().take(sample_limit) {
                speaker_warn_counter.add(item, 1);
            }
        }
        for item in &analysis.style_fatigue {
            if item.status == "WARN" {
                fatigue_counter.add(&item.family, item.count);
            }
        }

        let key_lists: [(&str, Vec<RankedItem>); 5] = [
            (
                "sentence_patterns",
                ranked_list(&analysis.sentence_patterns),
            ),
            ("sentence_starts", ranked_list(&analysis.sentence_starts)),
            (
                "judgement_endings",
                ranked_list(&analysis.judgement_endings),
            ),
            ("short_phrases", term_list(&analysis.short_phrases)),
            ("terms", term_list(&analysis.terms)),
        ];
        for (key, items) in key_lists {
            let ctr = counter_map
                .iter_mut()
                .find(|(name, _)| *name == key)
                .expect("counter 键已登记");
            for item in items.iter().take(sample_limit) {
                ctr.1.add(&item.label, item.count);
            }
        }
    }
    lines.push(String::new());

    let add_counter_section = |title: &str, counter: &Ctr, lines: &mut Vec<String>| {
        lines.push(format!("## {title}"));
        if counter.is_empty() {
            lines.push("- 无".to_string());
        } else {
            for (name, count) in counter.most_common(sample_limit * 2) {
                lines.push(format!("- `{name}` total=`{count}`"));
            }
        }
        lines.push(String::new());
    };

    add_counter_section(
        "Story-Wide Sentence Skeletons",
        &counter_map[0].1,
        &mut lines,
    );
    add_counter_section("Story-Wide Sentence Starts", &counter_map[1].1, &mut lines);
    add_counter_section(
        "Story-Wide Judgement Endings",
        &counter_map[2].1,
        &mut lines,
    );
    add_counter_section(
        "Story-Wide Structural Phrases",
        &counter_map[3].1,
        &mut lines,
    );
    add_counter_section(
        "Story-Wide Frequent Fragments",
        &counter_map[4].1,
        &mut lines,
    );
    add_counter_section("Story-Wide Style Fatigue", &fatigue_counter, &mut lines);
    add_counter_section("Story-Wide Scene Roles", &scene_role_counter, &mut lines);
    add_counter_section("Story-Wide Tone Signals", &tone_counter, &mut lines);
    add_counter_section(
        "Story-Wide Dialogue Emotions",
        &dialogue_emotion_counter,
        &mut lines,
    );
    add_counter_section("Story-Wide Character Voice", &speaker_counter, &mut lines);
    add_counter_section(
        "Story-Wide Voice Drift Signals",
        &speaker_warn_counter,
        &mut lines,
    );

    lines.join("\n") + "\n"
}

fn ranked_list(items: &[PhraseCount]) -> Vec<RankedItem> {
    items
        .iter()
        .map(|i| RankedItem {
            label: i.phrase.clone(),
            count: i.count,
        })
        .collect()
}

fn term_list(items: &[TermCount]) -> Vec<RankedItem> {
    items
        .iter()
        .map(|i| RankedItem {
            label: i.term.clone(),
            count: i.count,
        })
        .collect()
}

/// 收集章节 → 语料画像 → 逐章 `analyze_path` → 写
/// `profiles/*.md`（先全部章节）→ 按 story 写 `SUMMARY.md`。
/// 返回（退出码, 应打印路径序列）。
pub fn run(opts: &ProfileOptions) -> Result<(i32, Vec<PathBuf>)> {
    let files = collect_chapter_files(&opts.paths)?;
    if files.is_empty() {
        eprintln!("No draft chapter files found.");
        return Ok((1, Vec::new()));
    }

    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let ctx = DraftContext::new(rules)?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let corpus_profile = build_corpus_profile(&ctx, &ctx.corpus_paths_for_targets(&files), &files)?;

    let mut analyses: Vec<(PathBuf, Analysis)> = Vec::new();
    let mut printed: Vec<PathBuf> = Vec::new();
    for draft_path in &files {
        let analysis = analyze_path(
            &ctx,
            draft_path,
            &template_bank,
            ctx.draft_rules().tracked_terms.as_slice(),
            corpus_profile.as_ref(),
            opts.sample_limit,
        )
        .with_context(|| format!("无法分析章节 {}", draft_path.display()))?;
        analyses.push((draft_path.clone(), analysis.clone()));
        let out_path = profile_path_for(draft_path, opts.output_root.as_deref(), ctx.paths())?;
        write_text(
            &out_path,
            &build_profile_report(draft_path, &analysis, opts.sample_limit),
        )?;
        printed.push(out_path);
    }

    // 按父目录分组（首现序），再按 story 目录字典序处理。
    let mut groups: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for (index, (path, _)) in analyses.iter().enumerate() {
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(group) = groups.iter_mut().find(|g| g.0 == parent) {
            group.1.push(index);
        } else {
            groups.push((parent, vec![index]));
        }
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));

    for (story_dir, indexes) in &groups {
        let items: Vec<(PathBuf, &Analysis)> = indexes
            .iter()
            .map(|&i| (analyses[i].0.clone(), &analyses[i].1))
            .collect();
        let summary_path = profile_path_for(&items[0].0, opts.output_root.as_deref(), ctx.paths())?
            .parent()
            .context("profile 路径无父目录")?
            .join("SUMMARY.md");
        let summary = build_story_summary(story_dir, &items, opts.sample_limit);
        write_text(&summary_path, &summary)?;
        printed.push(summary_path);
    }
    Ok((0, printed))
}
