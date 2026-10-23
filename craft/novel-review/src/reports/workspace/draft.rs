//! draft 节收集与 `TEMPLATE_RESEARCH.md` 生成。

use super::*;

use super::tools::{
    rel_posix, summarize_counter, summarize_runs_local, summarize_story_convergences,
};
use super::trajectories::infer_template_deposition_target;

/// 收集 draft 节（复用 draft-stats / scorecard / catalog /
/// alignment / backlog / kit 模块，本函数只做等价编排与派生字段计算）。
pub(crate) fn collect_draft_section(
    novel_dir: &Path,
    sample_limit: usize,
    window_sizes: &[usize],
    ctx: &DraftContext,
    engine: &PlanEngine,
    output_root: Option<&Path>,
) -> Result<DraftSection> {
    let labels = &ctx.draft_rules().ending_labels;
    let drafts_dir = novel_dir.join(&ctx.paths().drafts);
    if !drafts_dir.exists() {
        return Ok(DraftSection {
            exists: false,
            files: 0,
            warnings: 0,
            stories: Vec::new(),
            workspace_templates: String::new(),
            template_targets: String::new(),
            template_research_path: novel_dir.join("draft-stats").join("TEMPLATE_RESEARCH.md"),
            template_catalog_summary_path: catalog::summary_path_for(novel_dir),
            template_catalog_json_path: catalog::json_path_for(novel_dir),
            template_learning_anchors: Vec::new(),
            template_writeback_queue: Vec::new(),
            alignment_summary: String::new(),
            alignment_mismatches: Vec::new(),
        });
    }
    let files = collect_chapter_files(std::slice::from_ref(&drafts_dir))?;
    if files.is_empty() {
        return Ok(DraftSection {
            exists: false,
            files: 0,
            warnings: 0,
            stories: Vec::new(),
            workspace_templates: String::new(),
            template_targets: String::new(),
            template_research_path: novel_dir.join("draft-stats").join("TEMPLATE_RESEARCH.md"),
            template_catalog_summary_path: catalog::summary_path_for(novel_dir),
            template_catalog_json_path: catalog::json_path_for(novel_dir),
            template_learning_anchors: Vec::new(),
            template_writeback_queue: Vec::new(),
            alignment_summary: String::new(),
            alignment_mismatches: Vec::new(),
        });
    }

    let corpus_paths = ctx.corpus_paths_for_targets(&files);
    let corpus_profile = crate::audit::draft::build_corpus_profile(ctx, &corpus_paths, &files)?;
    let template_bank = build_template_bank(ctx.draft_rules());
    let env = AnalysisEnv {
        ctx,
        template_bank: &template_bank,
        term_bank: &ctx.draft_rules().tracked_terms,
        corpus_profile: corpus_profile.as_ref(),
        sample_limit,
    };
    let analyses = env.analyze_chapters(&files)?;
    {
        use crate::stats::draft as draft_stats;
        draft_stats::build_single_reports(&env, &files, None, Some(&analyses), None)?;
        draft_stats::build_group_reports(
            &env,
            &files,
            window_sizes,
            None,
            Some(&analyses),
            labels,
        )?;
    }

    // grouped：story_dir -> 章分析（按父目录分组）。
    let mut grouped: BTreeMap<PathBuf, Vec<&(PathBuf, Analysis)>> = BTreeMap::new();
    for pair in &analyses {
        let parent = pair
            .0
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        grouped.entry(parent).or_default().push(pair);
    }

    let mut workspace_template_counter = Ctr::default();
    let mut deposition_counter = Ctr::default();
    let mut alignment_counter = Ctr::default();
    let mut alignment_mismatches: Vec<DraftDriftSample> = Vec::new();
    for (path, analysis) in &analyses {
        let alignment_result =
            alignment::build_plan_draft_alignment(engine, path, Some(novel_dir), analysis)?;
        if alignment_result.available {
            alignment_counter.add(
                &format!(
                    "chapter::{}->{}",
                    alignment_result
                        .plan_chapter_function
                        .as_deref()
                        .unwrap_or(""),
                    alignment_result
                        .draft_chapter_function
                        .as_deref()
                        .unwrap_or("")
                ),
                1,
            );
            alignment_counter.add(
                &format!(
                    "ending::{}->{}",
                    alignment_result
                        .plan_ending_function
                        .as_deref()
                        .unwrap_or(""),
                    alignment_result
                        .draft_ending_function
                        .as_deref()
                        .unwrap_or("")
                ),
                1,
            );
            if alignment_result.mismatch_count.unwrap_or(0) >= 1
                && alignment_mismatches.len() < DRIFT_SAMPLE_LIMIT * 4
            {
                alignment_mismatches.push(DraftDriftSample {
                    draft: rel_posix(path, &drafts_dir),
                    chapter: format!(
                        "{}->{}",
                        alignment_result
                            .plan_chapter_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .draft_chapter_function
                            .as_deref()
                            .unwrap_or("")
                    ),
                    ending: format!(
                        "{}->{}",
                        alignment_result
                            .plan_ending_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .draft_ending_function
                            .as_deref()
                            .unwrap_or("")
                    ),
                    score: alignment_result.mismatch_count.unwrap_or(0),
                });
            }
        }
        let mut seen: Vec<String> = Vec::new();
        for candidate in analysis.template_candidates.iter().take(20) {
            let key = format!("{}::{}", candidate.candidate_type, candidate.name);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            workspace_template_counter.add(&key, 1);
            deposition_counter.add(
                infer_template_deposition_target(&candidate.candidate_type, &candidate.name),
                1,
            );
        }
    }

    let mut stories: Vec<DraftStory> = Vec::new();
    for (story_dir, items) in &grouped {
        let mut ordered = items.clone();
        ordered.sort_by(|a, b| {
            b.1.summary
                .warn_sections
                .cmp(&a.1.summary.warn_sections)
                .then_with(|| b.1.summary.chars.cmp(&a.1.summary.chars))
                .then_with(|| {
                    let an =
                        a.0.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                    let bn =
                        b.0.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                    an.cmp(&bn)
                })
        });
        let (top_path, top_analysis) = ordered[0];
        let top_templates = top_analysis
            .template_candidates
            .iter()
            .take(3)
            .map(|c| format!("{} x{}", c.name, c.count))
            .collect::<Vec<_>>()
            .join(", ");
        let top_fatigue = top_analysis
            .style_fatigue
            .iter()
            .filter(|i| i.status == "WARN")
            .map(|i| format!("{} x{}", i.family, i.count))
            .collect::<Vec<_>>()
            .join(", ");

        let mut gates = Ctr::default();
        let mut recommendations = Ctr::default();
        let mut axis_totals = Ctr::default();
        let mut scene_role_counter = Ctr::default();
        let mut tone_counter = Ctr::default();
        let mut emotion_counter = Ctr::default();
        let mut speaker_counter = Ctr::default();
        let mut template_counter = Ctr::default();
        let mut story_alignment_counter = Ctr::default();
        let mut story_alignment_mismatches: Vec<String> = Vec::new();
        let mut voice_drifts: Vec<String> = Vec::new();
        let mut chapter_flow: Vec<String> = Vec::new();
        let mut ending_flow: Vec<String> = Vec::new();
        let mut ending_signal_flow: Vec<String> = Vec::new();
        let mut tone_flow: Vec<String> = Vec::new();
        let mut emotion_flow: Vec<String> = Vec::new();
        let mut ending_signal_counter = Ctr::default();

        for (path, analysis) in items {
            let axes = scorecard::build_axes(analysis, None, None, None);
            let (gate, _priority, recommendation) = scorecard::decide_gate(analysis, &axes);
            gates.add(&gate, 1);
            recommendations.add(&recommendation, 1);
            for axis in &axes {
                axis_totals.add(&axis.name, axis.score as usize);
            }
            scene_role_counter.add(&analysis.scene_map.dominant_role, 1);

            let dominant_tone = &analysis.tone_profile.dominant_tone;
            if !dominant_tone.is_empty() && dominant_tone != "none" {
                tone_counter.add(dominant_tone, 1);
            }
            tone_flow.push(if dominant_tone.is_empty() {
                "none".to_string()
            } else {
                dominant_tone.clone()
            });

            let dominant_emotion = &analysis.dialogue_emotions.dominant_emotion;
            if !dominant_emotion.is_empty() && dominant_emotion != "neutral" {
                emotion_counter.add(dominant_emotion, 1);
            }

            for speaker in analysis.character_voice.speakers.iter().take(4) {
                speaker_counter.add(&speaker.speaker, speaker.lines);
            }
            let cv = &analysis.character_voice;
            if cv.warn {
                voice_drifts.extend(cv.homogenized_pairs.iter().take(2).cloned());
            }

            emotion_flow.push(if dominant_emotion.is_empty() {
                "neutral".to_string()
            } else {
                dominant_emotion.clone()
            });
            let ending_signal = infer_ending_label(analysis, labels);
            ending_signal_counter.add(&ending_signal, 1);
            ending_signal_flow.push(ending_signal);

            let alignment_result =
                alignment::build_plan_draft_alignment(engine, path, Some(novel_dir), analysis)?;
            if alignment_result.available {
                story_alignment_counter.add(
                    &format!(
                        "{}->{}",
                        alignment_result
                            .plan_chapter_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .draft_chapter_function
                            .as_deref()
                            .unwrap_or("")
                    ),
                    1,
                );
                chapter_flow.push(
                    alignment_result
                        .draft_chapter_function
                        .as_deref()
                        .unwrap_or("")
                        .to_string(),
                );
                ending_flow.push(
                    alignment_result
                        .draft_ending_function
                        .as_deref()
                        .unwrap_or("")
                        .to_string(),
                );
                if alignment_result.mismatch_count.unwrap_or(0) >= 1
                    && story_alignment_mismatches.len() < 4
                {
                    story_alignment_mismatches.push(format!(
                        "{} chapter={}->{} ending={}->{}",
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        alignment_result
                            .plan_chapter_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .draft_chapter_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .plan_ending_function
                            .as_deref()
                            .unwrap_or(""),
                        alignment_result
                            .draft_ending_function
                            .as_deref()
                            .unwrap_or("")
                    ));
                }
            }

            let mut seen_templates: Vec<String> = Vec::new();
            for candidate in analysis.template_candidates.iter().take(20) {
                let key = format!("{}::{}", candidate.candidate_type, candidate.name);
                if seen_templates.contains(&key) {
                    continue;
                }
                seen_templates.push(key.clone());
                template_counter.add(&key, 1);
            }

            let scorecard_path = scorecard::scorecard_path_for(path, ctx.paths(), output_root)?;
            let report =
                scorecard::build_scorecard_report(engine, labels, path, analysis, None, None)?;
            input::write_text(&scorecard_path, &report)?;
        }

        let first_path = &items[0].0;
        let scorecard_summary_path =
            scorecard::scorecard_path_for(first_path, ctx.paths(), output_root)?
                .parent()
                .with_context(|| "scorecard path has no parent")?
                .join("SUMMARY.md");
        let pairs: Vec<(PathBuf, &Analysis)> = items.iter().map(|(p, a)| (p.clone(), a)).collect();
        let summary_md = scorecard::build_story_summary(story_dir, &pairs, &[], labels, engine)?;
        input::write_text(&scorecard_summary_path, &summary_md)?;

        let review_kit_summary_path =
            kit::review_kit_path_for(first_path, ctx.paths(), output_root)?;
        let template_backlog_summary_path =
            backlog::backlog_path_for(first_path, ctx.paths(), output_root)?;

        // avg_axis：按轴名排序 `{name} {round(total/n,2)}`（banker's + 浮点展示）。
        let avg_axis = {
            let mut entries = axis_totals.items();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let n = items.len().max(1);
            entries
                .iter()
                .map(|(name, total)| {
                    format!("{name} {}", float_repr(round2(*total as f64 / n as f64)))
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        // ending_signal_summary：按展示名计数（计数 + 首现序）。
        // —— 同显示名多 raw 时 dict 赋值「同位更新值、末次 raw 胜」，再 most_common(4) 空格连排。
        let ending_signal_summary = {
            let mut entries: Vec<(String, usize)> = Vec::new();
            for (name, count) in ending_signal_counter.items() {
                let display = ending_display(labels, &name);
                if let Some(slot) = entries.iter_mut().find(|(d, _)| *d == display) {
                    slot.1 = count;
                } else {
                    entries.push((display, count));
                }
            }
            let mut ordered = entries.clone();
            ordered.sort_by_key(|b| std::cmp::Reverse(b.1));
            let joined = ordered
                .into_iter()
                .take(4)
                .map(|(name, count)| format!("{name} x{count}"))
                .collect::<Vec<_>>()
                .join(" ");
            if joined.is_empty() {
                "无".to_string()
            } else {
                joined
            }
        };

        stories.push(DraftStory {
            story: rel_posix(story_dir, &drafts_dir),
            chapters: items.len(),
            warnings: items.iter().map(|(_, a)| a.summary.warn_sections).sum(),
            top_file: top_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            top_count: top_analysis.summary.warn_sections,
            top_templates: if top_templates.is_empty() {
                "无".into()
            } else {
                top_templates
            },
            top_fatigue: if top_fatigue.is_empty() {
                "无".into()
            } else {
                top_fatigue
            },
            gate_summary: ctr_sorted_join(&gates),
            recommendation_summary: ctr_sorted_join(&recommendations),
            avg_axis,
            scene_summary: summarize_counter(&scene_role_counter, 3),
            tone_summary: summarize_counter(&tone_counter, 3),
            emotion_summary: summarize_counter(&emotion_counter, 3),
            speaker_summary: summarize_counter(&speaker_counter, 4),
            template_summary: summarize_counter(&template_counter, 4),
            alignment_summary: summarize_counter(&story_alignment_counter, 3),
            voice_drifts: voice_drifts.into_iter().take(4).collect(),
            ending_signal_summary,
            alignment_mismatches: story_alignment_mismatches,
            chapter_runs: summarize_runs_local(&chapter_flow),
            ending_runs: summarize_runs_local(&ending_flow),
            ending_signal_flow: ending_flow_text(&ending_signal_flow, 8, labels),
            ending_signal_runs: summarize_runs(&ending_signal_flow, 2, 4, labels),
            trend_convergences: summarize_story_convergences(
                &ending_signal_flow,
                &tone_flow,
                &emotion_flow,
                labels,
            ),
            tone_runs: summarize_runs_local(&tone_flow),
            emotion_runs: summarize_runs_local(&emotion_flow),
            scorecard_summary_path,
            review_kit_summary_path,
            template_backlog_summary_path,
        });
    }

    // story payloads → TEMPLATE_RESEARCH.md + CATALOG。
    let mut story_payloads: Vec<Value> = Vec::new();
    for (story_dir, items) in &grouped {
        let pairs: Vec<(PathBuf, &Analysis)> = items.iter().map(|(p, a)| (p.clone(), a)).collect();
        let (_md, payload) = backlog::build_story_backlog(story_dir, &pairs)?;
        story_payloads.push(payload);
    }
    let template_research_path = novel_dir.join("draft-stats").join("TEMPLATE_RESEARCH.md");
    input::write_text(
        &template_research_path,
        &build_template_research_report(
            novel_dir,
            &grouped,
            &workspace_template_counter,
            &deposition_counter,
        ),
    )?;

    let catalog_value = catalog::build_catalog_payload(ctx, novel_dir, &story_payloads);
    let mut catalog_arrays: Vec<Vec<Value>> = Vec::new();
    for key in [
        "template_candidates",
        "template_families",
        "term_candidates",
        "keep_candidates",
        "deposition_targets",
        "learning_anchors",
        "writeback_queue",
    ] {
        catalog_arrays.push(
            catalog_value
                .get(key)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
    }
    let catalog_sections = catalog::CatalogSections {
        template_candidates: &catalog_arrays[0],
        template_families: &catalog_arrays[1],
        term_candidates: &catalog_arrays[2],
        keep_candidates: &catalog_arrays[3],
        deposition_targets: &catalog_arrays[4],
        learning_anchors: &catalog_arrays[5],
        writeback_queue: &catalog_arrays[6],
    };
    let catalog_markdown =
        catalog::build_catalog_markdown(novel_dir, story_payloads.len(), &catalog_sections);
    let summary_path = catalog::summary_path_for(novel_dir);
    let json_path = catalog::json_path_for(novel_dir);
    input::write_text(&summary_path, &catalog_markdown)?;
    input::write_json(&json_path, &catalog_value)?;

    Ok(DraftSection {
        exists: true,
        files: files.len(),
        warnings: analyses.iter().map(|(_, a)| a.summary.warn_sections).sum(),
        stories,
        workspace_templates: summarize_counter(&workspace_template_counter, 6),
        template_targets: summarize_counter(&deposition_counter, 4),
        template_research_path,
        template_catalog_summary_path: summary_path,
        template_catalog_json_path: json_path,
        template_learning_anchors: catalog_arrays[5].iter().take(4).cloned().collect(),
        template_writeback_queue: catalog_arrays[6].iter().take(5).cloned().collect(),
        alignment_summary: summarize_counter(&alignment_counter, 6),
        alignment_mismatches: alignment_mismatches
            .into_iter()
            .take(DRIFT_SAMPLE_LIMIT)
            .collect(),
    })
}

/// `sorted(counter.items())` → `{name} x{count}` 空格连排（`or "无"`）。
fn ctr_sorted_join(counter: &Ctr) -> String {
    let mut entries = counter.items();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let joined = entries
        .iter()
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        "无".to_string()
    } else {
        joined
    }
}

/// 写 `draft-stats/TEMPLATE_RESEARCH.md`。
fn build_template_research_report(
    novel_dir: &Path,
    grouped: &BTreeMap<PathBuf, Vec<&(PathBuf, Analysis)>>,
    workspace_counter: &Ctr,
    deposition_counter: &Ctr,
) -> String {
    let mut lines: Vec<String> = vec!["# Template Research".to_string(), String::new()];
    lines.push(format!(
        "- novel: `{}`",
        novel_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    lines.push(String::new());

    lines.push("## Workspace Candidates".to_string());
    if !workspace_counter.is_empty() {
        for (name, count) in workspace_counter.most_common(20) {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## Deposition Targets".to_string());
    if !deposition_counter.is_empty() {
        for (name, count) in deposition_counter.most_common_all() {
            lines.push(format!("- `{name}` x{count}"));
        }
    } else {
        lines.push("- 无".to_string());
    }
    lines.push(String::new());

    lines.push("## By Story".to_string());
    for (story_dir, items) in grouped {
        let mut template_counter = Ctr::default();
        for (_path, analysis) in items {
            let mut seen: Vec<String> = Vec::new();
            for candidate in analysis.template_candidates.iter().take(20) {
                let key = format!("{}::{}", candidate.candidate_type, candidate.name);
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key.clone());
                template_counter.add(&key, 1);
            }
        }
        lines.push(format!("- `{}`", story_dir.display()));
        if !template_counter.is_empty() {
            for (name, count) in template_counter.most_common(8) {
                lines.push(format!("  - `{name}` x{count}"));
            }
        } else {
            lines.push("  - 无".to_string());
        }
    }
    lines.push(String::new());
    lines.join("\n") + "\n"
}
