//! consistency 节派生行 builder（轨迹摘要/明细、叙事轨迹、关系对轨迹、沉淀目标推断）。

use super::*;

use super::tools::{chapter_order_from_path, summarize_counter};

/// fact 证据按章聚合的 timeline 行。
fn build_fact_timeline(
    conn: &Connection,
    novel_dir: &Path,
    story: &str,
    title: &str,
    fact_types: &[&str],
) -> Result<Vec<String>> {
    let types: Vec<String> = fact_types.iter().map(|s| s.to_string()).collect();
    let rows = consistency::query_story_tension_evidence(conn, story, title, 8 * 4, Some(&types))?;
    let mut grouped: BTreeMap<(i64, String), Vec<String>> = BTreeMap::new();
    for row in rows {
        let order = chapter_order_from_path(&row.path, novel_dir);
        grouped
            .entry(order)
            .or_default()
            .push(format!("{}:{}", row.fact_type, row.cue));
    }
    let mut timeline: Vec<String> = Vec::new();
    for ((_order, chapter_name), events) in &grouped {
        let mut deduped: Vec<&String> = Vec::new();
        for event in events {
            if !deduped.contains(&event) {
                deduped.push(event);
            }
        }
        timeline.push(format!(
            "{chapter_name} {}",
            deduped
                .into_iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("/")
        ));
        if timeline.len() >= 8 {
            break;
        }
    }
    Ok(timeline)
}

/// story 轨迹摘要（直接吃 typed 行）。
pub(crate) fn build_story_trajectory_summary(
    tension: &[TensionRow],
    goal: &[GoalTensionRow],
    relationship: &[RelationshipTensionRow],
) -> Vec<TrajectorySummaryRow> {
    #[derive(Default)]
    struct Bucket {
        state: Ctr,
        goal: Ctr,
        relationship: Ctr,
        samples: Vec<String>,
    }
    let mut by_story: BTreeMap<String, Bucket> = BTreeMap::new();
    for item in tension {
        let entry = by_story.entry(item.story.clone()).or_default();
        entry.state.add(&item.title, 1);
        if entry.samples.len() < 6 {
            let mut parts: Vec<String> = Vec::new();
            let inj_n = item.injury_negative.as_deref().unwrap_or("");
            let inj_s = item.injury_stable.as_deref().unwrap_or("");
            let eq_d = item.equipment_damaged.as_deref().unwrap_or("");
            let eq_a = item.equipment_active.as_deref().unwrap_or("");
            if !inj_n.is_empty() && !inj_s.is_empty() {
                parts.push(format!("injury={inj_n}->{inj_s}"));
            }
            if !eq_d.is_empty() && !eq_a.is_empty() {
                parts.push(format!("equipment={eq_d}->{eq_a}"));
            }
            entry
                .samples
                .push(format!("state `{}` {}", item.title, parts.join(" ; ")));
        }
    }
    for item in goal {
        let entry = by_story.entry(item.story.clone()).or_default();
        entry.goal.add(&item.title, 1);
        if entry.samples.len() < 6 {
            let mut parts: Vec<String> = Vec::new();
            let g = item.goal_assigned.as_deref().unwrap_or("");
            let c = item.goal_changed.as_deref().unwrap_or("");
            let done = item.goal_completed.as_deref().unwrap_or("");
            if !g.is_empty() {
                parts.push(format!("assigned={g}"));
            }
            if !c.is_empty() {
                parts.push(format!("changed={c}"));
            }
            if !done.is_empty() {
                parts.push(format!("completed={done}"));
            }
            entry
                .samples
                .push(format!("goal `{}` {}", item.title, parts.join(" ; ")));
        }
    }
    for item in relationship {
        let entry = by_story.entry(item.story.clone()).or_default();
        entry.relationship.add(&item.title, 1);
        if entry.samples.len() < 6 {
            entry.samples.push(format!(
                "relationship `{}` close={} ; distant={}",
                item.title,
                item.relationship_close.as_deref().unwrap_or(""),
                item.relationship_distant.as_deref().unwrap_or("")
            ));
        }
    }
    by_story
        .into_iter()
        .map(|(story, entry)| TrajectorySummaryRow {
            state_summary: summarize_counter(&entry.state, 3),
            goal_summary: summarize_counter(&entry.goal, 3),
            relationship_summary: summarize_counter(&entry.relationship, 3),
            story,
            samples: entry.samples.into_iter().take(4).collect(),
        })
        .collect()
}

/// story 轨迹明细。
pub(crate) fn build_story_trajectory_details(
    conn: &Connection,
    novel_dir: &Path,
    tension: &[TensionRow],
    goal: &[GoalTensionRow],
    relationship: &[RelationshipTensionRow],
) -> Result<Vec<TrajectoryDetailRow>> {
    let mut by_story: BTreeMap<String, Vec<TrajectoryDetailItem>> = BTreeMap::new();
    for item in tension {
        let inj_n = item.injury_negative.as_deref().unwrap_or("");
        let inj_s = item.injury_stable.as_deref().unwrap_or("");
        let eq_d = item.equipment_damaged.as_deref().unwrap_or("");
        let eq_a = item.equipment_active.as_deref().unwrap_or("");
        let fact_types: &[&str] = if !inj_n.is_empty() && !inj_s.is_empty() {
            &["injury_negative", "injury_stable"]
        } else if !eq_d.is_empty() && !eq_a.is_empty() {
            &["equipment_damaged", "equipment_active"]
        } else {
            continue;
        };
        let summary = if fact_types[0] == "injury_negative" {
            format!("injury={inj_n}->{inj_s}")
        } else {
            format!("equipment={eq_d}->{eq_a}")
        };
        let timeline = build_fact_timeline(conn, novel_dir, &item.story, &item.title, fact_types)?;
        by_story
            .entry(item.story.clone())
            .or_default()
            .push(TrajectoryDetailItem {
                title: item.title.clone(),
                kind: "state",
                summary,
                timeline,
            });
    }
    for item in goal {
        let g = item.goal_assigned.as_deref().unwrap_or("");
        let c = item.goal_changed.as_deref().unwrap_or("");
        let done = item.goal_completed.as_deref().unwrap_or("");
        let parts = [
            if g.is_empty() {
                None
            } else {
                Some(format!("assigned={g}"))
            },
            if c.is_empty() {
                None
            } else {
                Some(format!("changed={c}"))
            },
            if done.is_empty() {
                None
            } else {
                Some(format!("completed={done}"))
            },
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        let timeline = build_fact_timeline(
            conn,
            novel_dir,
            &item.story,
            &item.title,
            &["goal_assigned", "goal_changed", "goal_completed"],
        )?;
        by_story
            .entry(item.story.clone())
            .or_default()
            .push(TrajectoryDetailItem {
                title: item.title.clone(),
                kind: "goal",
                summary: parts.join(" ; "),
                timeline,
            });
    }
    for item in relationship {
        let summary = format!(
            "close={} ; distant={}",
            item.relationship_close.as_deref().unwrap_or(""),
            item.relationship_distant.as_deref().unwrap_or("")
        );
        let timeline = build_fact_timeline(
            conn,
            novel_dir,
            &item.story,
            &item.title,
            &["relationship_close", "relationship_distant"],
        )?;
        by_story
            .entry(item.story.clone())
            .or_default()
            .push(TrajectoryDetailItem {
                title: item.title.clone(),
                kind: "relationship",
                summary,
                timeline,
            });
    }
    Ok(by_story
        .into_iter()
        .map(|(story, items)| TrajectoryDetailRow {
            story,
            items: items.into_iter().take(8).collect(),
        })
        .collect())
}

/// 叙事轨迹行（drafts + consistency 派生）。
pub(crate) fn build_narrative_trajectory_rows(
    drafts: &DraftSection,
    consistency: &ConsistencySection,
) -> Vec<NarrativeRow> {
    let draft_rows: BTreeMap<String, &DraftStory> = drafts
        .stories
        .iter()
        .map(|s| {
            (
                s.story
                    .split_once('/')
                    .map(|(_, rest)| rest.to_string())
                    .unwrap_or_else(|| s.story.clone()),
                s,
            )
        })
        .collect();
    let consistency_rows: BTreeMap<String, &TrajectorySummaryRow> = consistency
        .story_trajectories
        .iter()
        .map(|r| (r.story.clone(), r))
        .collect();
    let detail_rows: BTreeMap<String, &TrajectoryDetailRow> = consistency
        .story_trajectory_details
        .iter()
        .map(|r| (r.story.clone(), r))
        .collect();

    let mut story_keys: BTreeSet<&String> = BTreeSet::new();
    story_keys.extend(draft_rows.keys());
    story_keys.extend(consistency_rows.keys());
    story_keys.extend(detail_rows.keys());

    story_keys
        .into_iter()
        .map(|story| {
            let draft = draft_rows.get(story).copied();
            let consistency_summary = consistency_rows.get(story).copied();
            let consistency_detail = detail_rows.get(story).copied();
            let mut trajectory_parts: Vec<String> = Vec::new();
            if let Some(d) = draft {
                if !d.speaker_summary.is_empty() && d.speaker_summary != "无" {
                    trajectory_parts.push(format!("speakers={}", d.speaker_summary));
                }
                if !d.tone_runs.is_empty() {
                    trajectory_parts.push(format!(
                        "tone_runs={}",
                        d.tone_runs
                            .iter()
                            .take(2)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
                if !d.emotion_runs.is_empty() {
                    trajectory_parts.push(format!(
                        "emotion_runs={}",
                        d.emotion_runs
                            .iter()
                            .take(2)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
                if !d.chapter_runs.is_empty() {
                    trajectory_parts.push(format!(
                        "chapter_runs={}",
                        d.chapter_runs
                            .iter()
                            .take(2)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
                if !d.voice_drifts.is_empty() {
                    trajectory_parts.push(format!(
                        "voice={}",
                        d.voice_drifts
                            .iter()
                            .take(2)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                }
            }
            if let Some(c) = consistency_summary {
                if !c.goal_summary.is_empty() && c.goal_summary != "无" {
                    trajectory_parts.push(format!("goal={}", c.goal_summary));
                }
                if !c.relationship_summary.is_empty() && c.relationship_summary != "无" {
                    trajectory_parts.push(format!("relationship={}", c.relationship_summary));
                }
                if !c.state_summary.is_empty() && c.state_summary != "无" {
                    trajectory_parts.push(format!("state={}", c.state_summary));
                }
            }
            let detail_samples: Vec<String> = consistency_detail
                .map(|d| {
                    d.items
                        .iter()
                        .take(3)
                        .map(|item| {
                            format!(
                                "{}:{} {}",
                                item.kind,
                                item.title,
                                item.timeline
                                    .iter()
                                    .take(2)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" -> ")
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            NarrativeRow {
                story: story.to_string(),
                summary: {
                    let joined = trajectory_parts.join(" ; ");
                    if joined.is_empty() {
                        "无".to_string()
                    } else {
                        joined
                    }
                },
                details: detail_samples,
            }
        })
        .collect()
}

/// 关系对轨迹（每 story 上限 4 条）。
pub(crate) fn build_relationship_pair_trajectories(
    novel_dir: &Path,
    pair_rows: &[PairRow],
    limit_per_story: usize,
) -> Vec<PairTrajectoryRow> {
    let mut grouped: BTreeMap<String, BTreeMap<String, Vec<&PairRow>>> = BTreeMap::new();
    for row in pair_rows {
        let pair = format!("{}~{}", row.left_title, row.right_title);
        grouped
            .entry(row.story.clone())
            .or_default()
            .entry(pair)
            .or_default()
            .push(row);
    }

    let mut results: Vec<PairTrajectoryRow> = Vec::new();
    for (story, pairs) in &grouped {
        let mut items: Vec<PairTrajectoryItem> = Vec::new();
        for (pair, rows) in pairs {
            let mut ordered_rows: Vec<&PairRow> = rows.to_vec();
            ordered_rows.sort_by_key(|row| chapter_order_from_path(&row.path, novel_dir));
            let mut timeline: Vec<String> = Vec::new();
            let mut close_count = 0usize;
            let mut distant_count = 0usize;
            for row in &ordered_rows {
                let chapter_name = chapter_order_from_path(&row.path, novel_dir).1;
                let mut parts: Vec<String> = Vec::new();
                if !row.close_cues.is_empty() {
                    close_count += 1;
                    parts.push(format!("close={}", row.close_cues));
                }
                if !row.distant_cues.is_empty() {
                    distant_count += 1;
                    parts.push(format!("distant={}", row.distant_cues));
                }
                if !parts.is_empty() {
                    timeline.push(format!("{chapter_name} {}", parts.join(" ; ")));
                }
            }
            if timeline.is_empty() {
                continue;
            }
            items.push(PairTrajectoryItem {
                pair: pair.clone(),
                summary: format!("close_hits={close_count} distant_hits={distant_count}"),
                timeline: timeline.into_iter().take(4).collect(),
            });
        }
        if !items.is_empty() {
            results.push(PairTrajectoryRow {
                story: story.clone(),
                items: items.into_iter().take(limit_per_story).collect(),
            });
        }
    }
    results
}

/// 模板沉淀目标推断。
pub(crate) fn infer_template_deposition_target(
    candidate_type: &str,
    candidate_name: &str,
) -> &'static str {
    if matches!(
        candidate_type,
        "dialogue" | "dialogue_axis_gap" | "dialogue_emotion"
    ) {
        return "skills/review-guide.md";
    }
    if matches!(
        candidate_type,
        "tracked_term" | "tracked_term_window" | "learned_filter"
    ) {
        return "configs/rules/review.yaml#draft.tracked_terms";
    }
    if matches!(
        candidate_type,
        "scene_map" | "battle_profile" | "viewpoint_profile"
    ) {
        return "configs/rules/review.yaml#draft.template_rules";
    }
    if matches!(
        candidate_type,
        "sentence_pattern" | "short_phrase" | "aa_bb_pattern" | "custom_template"
    ) {
        return "configs/rules/review.yaml#draft.template_rules";
    }
    if candidate_type == "ending" {
        return "novel1/rules/draft.md";
    }
    if matches!(
        candidate_name,
        "场面功能失衡" | "动作链缺结果" | "视角锚点漂移"
    ) {
        return "configs/rules/review.yaml#draft.template_rules";
    }
    "configs/rules/review.yaml#draft.template_rules"
}
