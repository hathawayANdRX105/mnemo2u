//! concept / plan / consistency 节收集（`collect_*_section`）。

use super::*;

use super::tools::{summarize_counter, summarize_runs_local};
use super::trajectories::{
    build_relationship_pair_trajectories, build_story_trajectory_details,
    build_story_trajectory_summary,
};

/// 收集 concept 节。
pub(crate) fn collect_concept_section(novel_dir: &Path) -> Result<ConceptSection> {
    let cards_dir = novel_dir.join("concept").join("cards");
    if !cards_dir.exists() {
        return Ok(ConceptSection {
            exists: false,
            files: 0,
            warnings: 0,
            summary_paths: Vec::new(),
            categories: Vec::new(),
        });
    }
    let files = concept_stats::collect_targets(std::slice::from_ref(&cards_dir), false);
    let reports = concept_stats::build_single_reports(&files)?;
    let summary_paths = concept_stats::build_directory_summaries(&reports)?;
    let mut grouped: BTreeMap<String, Vec<concept_stats::Report>> = BTreeMap::new();
    for report in &reports {
        let key = report
            .0
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        grouped.entry(key).or_default().push(report.clone());
    }
    let mut categories: Vec<ConceptCategory> = Vec::new();
    for (category, items) in &grouped {
        let mut ordered = items.clone();
        ordered.sort_by(|a, b| {
            b.1.len().cmp(&a.1.len()).then_with(|| {
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
        let total_warnings = items.iter().map(|(_, warnings)| warnings.len()).sum();
        let (top_file, top_warnings) = ordered[0].clone();
        let mut kind_counter = Ctr::default();
        for w in &top_warnings {
            kind_counter.add(&w.kind, 1);
        }
        let joined = kind_counter
            .most_common(3)
            .iter()
            .map(|(kind, count)| format!("{kind} x{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        categories.push(ConceptCategory {
            category: category.clone(),
            cards: items.len(),
            warnings: total_warnings,
            top_file: top_file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            top_count: top_warnings.len(),
            top_kinds: if joined.is_empty() {
                "无".into()
            } else {
                joined
            },
        });
    }
    Ok(ConceptSection {
        exists: true,
        files: files.len(),
        warnings: reports.iter().map(|(_, w)| w.len()).sum(),
        summary_paths,
        categories,
    })
}

/// 收集 plan 节。
pub(crate) fn collect_plan_section(novel_dir: &Path, engine: &PlanEngine) -> Result<PlanSection> {
    let plan_dirs: Vec<PathBuf> = engine
        .paths()
        .plan_dirs()
        .iter()
        .map(|name| novel_dir.join(name))
        .filter(|path| path.exists())
        .collect();
    if plan_dirs.is_empty() {
        return Ok(PlanSection {
            exists: false,
            files: 0,
            warnings: 0,
            summary_paths: Vec::new(),
            plan_types: Vec::new(),
            chapter_function_distribution: String::new(),
            ending_function_distribution: String::new(),
            story_trends: Vec::new(),
        });
    }
    let files = plan_stats::collect_targets(&plan_dirs);
    let reports = plan_stats::build_single_reports(engine, &files, None, None)?;
    let summary_paths = plan_stats::build_directory_summaries(engine, &reports, None)?;

    let mut grouped: BTreeMap<String, Vec<plan_stats::Report>> = BTreeMap::new();
    let mut chapter_function_counter = Ctr::default();
    let mut ending_function_counter = Ctr::default();
    let mut chapter_plan_by_story: BTreeMap<String, Vec<(PathBuf, String, String)>> =
        BTreeMap::new();
    for (path, plan_type, warnings) in &reports {
        grouped.entry(plan_type.clone()).or_default().push((
            path.clone(),
            plan_type.clone(),
            warnings.clone(),
        ));
        if plan_type == "chapter-plan" {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("无法读取大纲文件 {}", path.display()))?;
            let lines: Vec<&str> = text.lines().collect();
            let headings = plan_audit::parse_headings(&lines);
            let sections = plan_audit::collect_section_lines(&lines, &headings);
            let (_name, chapter_function_section) =
                plan_audit::find_section(&sections, &["本章功能"]);
            let bullets: Vec<String> = plan_audit::bullet_lines(chapter_function_section)
                .iter()
                .map(|line| line.1.clone())
                .collect();
            let chapter_function_text = {
                let joined = bullets.join(" ");
                if joined.is_empty() {
                    plan_audit::section_text(chapter_function_section)
                } else {
                    joined
                }
            };
            let chapter_function = plan_audit::detect_function_label(
                &chapter_function_text,
                engine.chapter_function_rules(),
            );
            chapter_function_counter.add(&chapter_function, 1);

            let ending_choices: Vec<&str> = engine
                .chapter_ending_group()
                .iter()
                .map(|s| s.as_str())
                .collect();
            let (_ending_name, ending_section) =
                plan_audit::find_section(&sections, &ending_choices);
            let ending_bullets: Vec<String> = plan_audit::bullet_lines(ending_section)
                .iter()
                .map(|line| line.1.clone())
                .collect();
            let ending_text = {
                let joined = ending_bullets.join(" ");
                if joined.is_empty() {
                    plan_audit::section_text(ending_section)
                } else {
                    joined
                }
            };
            let ending_function =
                plan_audit::detect_function_label(&ending_text, engine.ending_function_rules());
            ending_function_counter.add(&ending_function, 1);

            let (_doc_type, _arc, story, _chapter) =
                consistency::classify_document(path, novel_dir);
            let story_key = story
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "story1".to_string());
            chapter_plan_by_story.entry(story_key).or_default().push((
                path.clone(),
                chapter_function,
                ending_function,
            ));
        }
    }

    let mut plan_types: Vec<PlanTypeInfo> = Vec::new();
    for (plan_type, items) in &grouped {
        let mut ordered = items.clone();
        ordered.sort_by(|a, b| {
            b.2.len().cmp(&a.2.len()).then_with(|| {
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
        let total_warnings = items.iter().map(|(_, _, w)| w.len()).sum();
        let (top_file, _top_type, top_warnings) = &ordered[0];
        let mut kind_counter = Ctr::default();
        for w in top_warnings {
            kind_counter.add(&w.kind, 1);
        }
        let joined = kind_counter
            .most_common(3)
            .iter()
            .map(|(kind, count)| format!("{kind} x{count}"))
            .collect::<Vec<_>>()
            .join(", ");
        plan_types.push(PlanTypeInfo {
            plan_type: plan_type.clone(),
            files: items.len(),
            warnings: total_warnings,
            top_file: top_file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            top_count: top_warnings.len(),
            top_kinds: if joined.is_empty() {
                "无".into()
            } else {
                joined
            },
        });
    }

    let mut story_trends: Vec<StoryTrend> = Vec::new();
    for (story, rows) in &chapter_plan_by_story {
        let mut ordered_rows = rows.clone();
        ordered_rows.sort_by_key(|item| {
            item.0
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let chapter_labels: Vec<String> = ordered_rows.iter().map(|item| item.1.clone()).collect();
        let ending_labels: Vec<String> = ordered_rows.iter().map(|item| item.2.clone()).collect();
        story_trends.push(StoryTrend {
            story: story.clone(),
            chapter_flow: {
                let joined = chapter_labels
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" -> ");
                if chapter_labels.is_empty() {
                    "无".into()
                } else {
                    joined
                }
            },
            ending_flow: {
                let joined = ending_labels
                    .iter()
                    .take(8)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" -> ");
                if ending_labels.is_empty() {
                    "无".into()
                } else {
                    joined
                }
            },
            chapter_runs: summarize_runs_local(&chapter_labels),
            ending_runs: summarize_runs_local(&ending_labels),
        });
    }

    Ok(PlanSection {
        exists: true,
        files: files.len(),
        warnings: reports.iter().map(|(_, _, w)| w.len()).sum(),
        summary_paths,
        plan_types,
        chapter_function_distribution: summarize_counter(&chapter_function_counter, 6),
        ending_function_distribution: summarize_counter(&ending_function_counter, 6),
        story_trends,
    })
}

/// 收集 consistency 节。
pub(crate) fn collect_consistency_section(
    novel_dir: &Path,
    paths: &crate::config::PathsConfig,
) -> Result<ConsistencySection> {
    let db_path = paths.consistency_db_path(novel_dir);
    let feedback_path = novel_dir
        .join(&paths.research)
        .join("consistency")
        .join("review-feedback.jsonl");
    consistency::build_index(novel_dir, &db_path, paths)?;
    let conn = consistency::open_db(&db_path)?;
    let alignment_rows = consistency::query_story_alignment_rows(&conn, 200)?;
    let tension_rows = consistency::query_story_tension_rows(&conn, 200)?;
    let goal_rows = consistency::query_story_goal_tension_rows(&conn, 200)?;
    let relationship_rows = consistency::query_story_relationship_tension_rows(&conn, 200)?;
    let pair_rows = consistency::query_story_relationship_pair_rows(&conn, 800)?;
    let conflict_rows = consistency::collect_conflict_rows(&conn, 200, Some(&feedback_path))?;
    let feedback_summary = consistency::summarize_feedback(&conn, &feedback_path, 200, None)?;
    let story_trajectories =
        build_story_trajectory_summary(&tension_rows, &goal_rows, &relationship_rows);
    let story_trajectory_details = build_story_trajectory_details(
        &conn,
        novel_dir,
        &tension_rows,
        &goal_rows,
        &relationship_rows,
    )?;
    let relationship_pair_trajectories =
        build_relationship_pair_trajectories(novel_dir, &pair_rows, 4);
    drop(conn);

    Ok(ConsistencySection {
        db_path,
        feedback_path,
        feedback_entries: feedback_summary.entries.len(),
        feedback_decisions: feedback_summary.decision_counter.most_common_all(),
        feedback_facets: feedback_summary.facet_counter.most_common(6),
        feedback_backlog: feedback_summary.backlog,
        feedback_pending: feedback_summary.unresolved.len(),
        feedback_pending_samples: feedback_summary.unresolved.into_iter().take(6).collect(),
        story_alignment: alignment_rows,
        story_tension: tension_rows,
        story_goal_tension: goal_rows,
        story_relationship_tension: relationship_rows,
        story_conflicts: conflict_rows,
        story_trajectories,
        story_trajectory_details,
        relationship_pair_trajectories,
    })
}
