//! 冲突候选收集、反馈写入与 story 冲突快照。

use super::*;
/// `apply_feedback`。
pub fn apply_feedback(
    mut rows: Vec<ConflictRow>,
    feedback_entries: &BTreeMap<String, FeedbackRecord>,
) -> Vec<ConflictRow> {
    for row in &mut rows {
        let key = conflict_key(row);
        let Some(feedback) = feedback_entries.get(&key) else {
            continue;
        };
        row.feedback_decision = record_str(feedback, "decision");
        row.feedback_facet = record_str(feedback, "facet");
        row.feedback_note = record_str(feedback, "note");
        row.feedback_updated_at = record_str(feedback, "updated_at");
    }
    rows
}

/// `query_conflict_rows`。
pub fn query_conflict_rows(conn: &Connection, limit: i64) -> Result<Vec<ConflictRow>> {
    let mut rows: Vec<ConflictRow> = Vec::new();

    for row in query_story_tension_rows(conn, limit)? {
        if !row.injury_negative.as_deref().unwrap_or("").is_empty()
            && !row.injury_stable.as_deref().unwrap_or("").is_empty()
        {
            let fact_types = vec!["injury_negative".to_string(), "injury_stable".to_string()];
            let support = query_fact_support_summary(conn, &row.story, &row.title, &fact_types)?;
            let (confidence, support_note) = score_fact_confidence(&support);
            if confidence == "low" {
                continue;
            }
            rows.push(ConflictRow {
                category: "injury_state_jump".into(),
                story: row.story.clone(),
                title: row.title.clone(),
                entity_category: row.category.clone(),
                summary: format!(
                    "injury={} -> {}",
                    row.injury_negative.as_deref().unwrap_or(""),
                    row.injury_stable.as_deref().unwrap_or("")
                ),
                evidence_kind: EvidenceKind::Fact,
                fact_types,
                confidence,
                support_note,
                ..Default::default()
            });
        }
        if !row.equipment_damaged.as_deref().unwrap_or("").is_empty()
            && !row.equipment_active.as_deref().unwrap_or("").is_empty()
        {
            let fact_types = vec![
                "equipment_damaged".to_string(),
                "equipment_active".to_string(),
            ];
            let support = query_fact_support_summary(conn, &row.story, &row.title, &fact_types)?;
            let (confidence, support_note) = score_fact_confidence(&support);
            if confidence == "low" {
                continue;
            }
            rows.push(ConflictRow {
                category: "equipment_state_jump".into(),
                story: row.story.clone(),
                title: row.title.clone(),
                entity_category: row.category.clone(),
                summary: format!(
                    "equipment={} -> {}",
                    row.equipment_damaged.as_deref().unwrap_or(""),
                    row.equipment_active.as_deref().unwrap_or("")
                ),
                evidence_kind: EvidenceKind::Fact,
                fact_types,
                confidence,
                support_note,
                ..Default::default()
            });
        }
    }

    for row in query_story_goal_tension_rows(conn, limit)? {
        let fact_types = vec![
            "goal_assigned".to_string(),
            "goal_changed".to_string(),
            "goal_completed".to_string(),
        ];
        let support = query_fact_support_summary(conn, &row.story, &row.title, &fact_types)?;
        let (confidence, support_note) = score_fact_confidence(&support);
        if confidence == "low" {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        if !row.goal_assigned.as_deref().unwrap_or("").is_empty() {
            parts.push(format!(
                "assigned={}",
                row.goal_assigned.as_deref().unwrap_or("")
            ));
        }
        if !row.goal_changed.as_deref().unwrap_or("").is_empty() {
            parts.push(format!(
                "changed={}",
                row.goal_changed.as_deref().unwrap_or("")
            ));
        }
        if !row.goal_completed.as_deref().unwrap_or("").is_empty() {
            parts.push(format!(
                "completed={}",
                row.goal_completed.as_deref().unwrap_or("")
            ));
        }
        rows.push(ConflictRow {
            category: "goal_state_drift".into(),
            story: row.story,
            title: row.title,
            entity_category: row.category,
            summary: parts.join(" ; "),
            evidence_kind: EvidenceKind::Fact,
            fact_types,
            confidence,
            support_note,
            ..Default::default()
        });
    }

    for row in query_story_relationship_tension_rows(conn, limit)? {
        let fact_types = vec![
            "relationship_close".to_string(),
            "relationship_distant".to_string(),
        ];
        let support = query_fact_support_summary(conn, &row.story, &row.title, &fact_types)?;
        let (confidence, support_note) = score_fact_confidence(&support);
        if confidence == "low" {
            continue;
        }
        rows.push(ConflictRow {
            category: "relationship_tone_shift".into(),
            story: row.story,
            title: row.title,
            entity_category: row.category,
            summary: format!(
                "close={} ; distant={}",
                row.relationship_close.as_deref().unwrap_or(""),
                row.relationship_distant.as_deref().unwrap_or("")
            ),
            evidence_kind: EvidenceKind::Fact,
            fact_types,
            confidence,
            support_note,
            ..Default::default()
        });
    }

    for row in query_story_alias_drift_rows(conn, limit)? {
        let plan_aliases = row.plan_aliases.clone().unwrap_or_default();
        let draft_aliases = row.draft_aliases.clone().unwrap_or_default();
        let (confidence, support_note) = score_alias_confidence(&plan_aliases, &draft_aliases);
        if confidence == "low" {
            continue;
        }
        rows.push(ConflictRow {
            category: "alias_register_drift".into(),
            story: row.story,
            title: row.title,
            entity_category: row.category,
            summary: format!("plan={} ; draft={}", plan_aliases, draft_aliases),
            evidence_kind: EvidenceKind::Alias,
            fact_types: Vec::new(),
            confidence,
            support_note,
            ..Default::default()
        });
    }

    for row in query_story_alignment_gap_rows(conn, limit)? {
        let mut parts: Vec<String> = Vec::new();
        if let Some(plan_only) = &row.plan_only_entities {
            parts.push(format!("plan_only={plan_only}"));
        }
        if let Some(draft_only) = &row.draft_only_entities {
            parts.push(format!("draft_only={draft_only}"));
        }
        let summary = parts.join(" ; ");
        let (confidence, support_note) = score_alignment_confidence(&summary);
        if confidence == "low" {
            continue;
        }
        rows.push(ConflictRow {
            category: "plan_draft_entity_drift".into(),
            story: row.story,
            title: "-".into(),
            entity_category: "story".into(),
            summary,
            evidence_kind: EvidenceKind::Alignment,
            fact_types: Vec::new(),
            confidence,
            support_note,
            ..Default::default()
        });
    }

    let confidence_rank = |value: &str| -> i64 {
        match value {
            "high" => 0,
            "medium" => 1,
            "low" => 2,
            _ => 9,
        }
    };
    rows.sort_by(|a, b| {
        confidence_rank(&a.confidence)
            .cmp(&confidence_rank(&b.confidence))
            .then_with(|| a.story.cmp(&b.story))
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.title.cmp(&b.title))
    });
    rows.truncate(limit as usize);
    Ok(rows)
}

/// `collect_conflict_rows`。
pub fn collect_conflict_rows(
    conn: &Connection,
    limit: i64,
    feedback_path: Option<&Path>,
) -> Result<Vec<ConflictRow>> {
    let rows = query_conflict_rows(conn, limit)?;
    let Some(feedback_path) = feedback_path else {
        return Ok(rows);
    };
    let entries = load_feedback_entries(feedback_path)?;
    Ok(apply_feedback(rows, &entries))
}

/// `write_feedback` 请求参数。
pub struct FeedbackRequest {
    pub category: String,
    pub story: String,
    pub title: String,
    pub decision: String,
    pub facet: String,
    pub note: String,
    pub summary_contains: Option<String>,
}

/// `write_feedback`：成功返回 `Ok(None)`；校验/匹配失败返回 `Ok(Some(msg))`
/// （校验/匹配失败消息走 stderr + 退出码 1）。
pub fn write_feedback(
    conn: &Connection,
    feedback_path: &Path,
    req: &FeedbackRequest,
) -> Result<Option<String>> {
    if !FEEDBACK_DECISIONS.contains(&req.decision.as_str()) {
        return Ok(Some(format!("Unknown decision: {}", req.decision)));
    }
    if !req.facet.is_empty() && !FEEDBACK_FACETS.contains(&req.facet.as_str()) {
        return Ok(Some(format!("Unknown facet: {}", req.facet)));
    }
    let rows = query_conflict_rows(conn, 1000)?;
    let matched: Vec<&ConflictRow> = rows
        .iter()
        .filter(|row| {
            row.category == req.category
                && row.story == req.story
                && row.title == req.title
                && req
                    .summary_contains
                    .as_deref()
                    .is_none_or(|needle| row.summary.contains(needle))
        })
        .collect();
    if matched.is_empty() {
        return Ok(Some("No matching conflict row found.".to_string()));
    }
    if matched.len() > 1 {
        return Ok(Some(
            "Multiple conflict rows matched. Add --summary-contains to disambiguate.".to_string(),
        ));
    }
    let row = matched[0];
    let mut entry: FeedbackRecord = JsonMap::new();
    let updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    entry.insert("conflict_key".into(), conflict_key(row).into());
    entry.insert("category".into(), req.category.clone().into());
    entry.insert("story".into(), req.story.clone().into());
    entry.insert("title".into(), req.title.clone().into());
    entry.insert("entity_category".into(), row.entity_category.clone().into());
    entry.insert("summary".into(), row.summary.clone().into());
    entry.insert("decision".into(), req.decision.clone().into());
    entry.insert("facet".into(), req.facet.clone().into());
    entry.insert("note".into(), req.note.clone().into());
    entry.insert("updated_at".into(), updated_at.into());
    append_feedback_entry(feedback_path, &entry)?;
    Ok(None)
}

/// `summarize_feedback`。
pub fn summarize_feedback(
    conn: &Connection,
    feedback_path: &Path,
    limit: i64,
    story: Option<&str>,
) -> Result<FeedbackSummary> {
    let feedback_entries = load_feedback_entries(feedback_path)?;
    let feedback_history = read_feedback_history(feedback_path)?;
    let all_conflict_rows = collect_conflict_rows(conn, limit, Some(feedback_path))?;
    let conflict_rows: Vec<ConflictRow> = all_conflict_rows
        .into_iter()
        .filter(|row| match story {
            None => true,
            Some(s) => row.story == s,
        })
        .collect();
    let scoped_history = filter_feedback_history_by_story(&feedback_history, story);

    let mut decision_counter = Ctr::default();
    let mut category_counter = Ctr::default();
    let mut story_counter = Ctr::default();
    let mut facet_counter = Ctr::default();
    let mut unresolved: Vec<ConflictRow> = Vec::new();

    for row in &conflict_rows {
        if !row.feedback_decision.is_empty() {
            decision_counter.add(&row.feedback_decision, 1);
            category_counter.add(&format!("{}::{}", row.category, row.feedback_decision), 1);
            story_counter.add(&format!("{}::{}", row.story, row.feedback_decision), 1);
            if !row.feedback_facet.is_empty() {
                facet_counter.add(
                    &format!("{}::{}", row.feedback_facet, row.feedback_decision),
                    1,
                );
            }
        } else {
            unresolved.push(row.clone());
        }
    }

    let db_root = parent_or_dot(feedback_path);
    let resolved_root = resolve_db_path(&db_root);
    let pending_actions = build_pending_review_actions(&resolved_root, &unresolved, 8);
    let mut unresolved_by_story = Ctr::default();
    for row in &unresolved {
        unresolved_by_story.add(&row.story, 1);
    }
    Ok(FeedbackSummary {
        entries: feedback_entries,
        history: feedback_history,
        conflict_rows,
        decision_counter,
        category_counter,
        story_counter,
        facet_counter,
        unresolved,
        pending_actions,
        backlog: build_feedback_backlog(&scoped_history),
        unresolved_by_story,
        story_filter: story.map(str::to_string),
    })
}

/// `build_story_conflict_snapshot_from_path`（`limit` 缺省 200）。
pub fn build_story_conflict_snapshot_from_path(
    draft_path: &Path,
    limit: i64,
) -> Result<StoryConflictSnapshot> {
    let resolved_path = if draft_path.exists() {
        std::fs::canonicalize(draft_path)?
    } else {
        draft_path.to_path_buf()
    };
    let novel_dir = match find_novel_dir(&resolved_path) {
        Some(dir) => dir,
        None => return Ok(unavailable_snapshot("novel_dir_not_found")),
    };

    let db_path = novel_dir
        .join("research")
        .join("consistency")
        .join("consistency.sqlite3");
    let feedback_path = default_feedback_path_from_db(&db_path);
    if !db_path.exists() {
        return Ok(StoryConflictSnapshot {
            available: false,
            reason: Some("db_not_found".into()),
            novel_dir: Some(novel_dir),
            db_path: Some(db_path),
            feedback_path: Some(feedback_path),
            story: None,
            review_queue_command: None,
            feedback_summary_command: None,
            rows: Vec::new(),
            decision_counter: Ctr::default(),
            category_counter: Ctr::default(),
            facet_counter: Ctr::default(),
            pending_rows: Vec::new(),
            pending_count: 0,
            pending_actions: Vec::new(),
            global_feedback_backlog: Vec::new(),
        });
    }

    let (doc_type, _arc, story, _chapter) = classify_document(&resolved_path, &novel_dir);
    let story = if doc_type != "drafts" { None } else { story };
    let story = match story {
        Some(story) => story,
        None => {
            return Ok(StoryConflictSnapshot {
                available: false,
                reason: Some("story_not_found".into()),
                novel_dir: Some(novel_dir),
                db_path: Some(db_path),
                feedback_path: Some(feedback_path),
                story: None,
                review_queue_command: None,
                feedback_summary_command: None,
                rows: Vec::new(),
                decision_counter: Ctr::default(),
                category_counter: Ctr::default(),
                facet_counter: Ctr::default(),
                pending_rows: Vec::new(),
                pending_count: 0,
                pending_actions: Vec::new(),
                global_feedback_backlog: Vec::new(),
            });
        }
    };

    let conn = open_db(&db_path)?;
    let rows = collect_conflict_rows(&conn, limit, Some(&feedback_path))?;
    let rows: Vec<ConflictRow> = rows.into_iter().filter(|row| row.story == story).collect();
    let feedback_history = read_feedback_history(&feedback_path)?;

    let mut decision_counter = Ctr::default();
    let mut facet_counter = Ctr::default();
    let mut category_counter = Ctr::default();
    let mut pending_rows: Vec<ConflictRow> = Vec::new();
    for row in &rows {
        if !row.feedback_decision.is_empty() {
            decision_counter.add(&row.feedback_decision, 1);
            category_counter.add(&format!("{}::{}", row.category, row.feedback_decision), 1);
            if !row.feedback_facet.is_empty() {
                facet_counter.add(&row.feedback_facet, 1);
            }
        } else {
            pending_rows.push(row.clone());
        }
    }

    let novel_dir_name = novel_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let pending_actions = build_pending_review_actions(&db_path, &pending_rows, 4);
    Ok(StoryConflictSnapshot {
        available: true,
        reason: None,
        novel_dir: Some(novel_dir),
        db_path: Some(db_path),
        feedback_path: Some(feedback_path),
        story: Some(story.clone()),
        review_queue_command: Some(format!(
            "novel-review consistency review-queue {novel_dir_name} --story {story}",
        )),
        feedback_summary_command: Some(format!(
            "novel-review consistency feedback-summary {novel_dir_name} --story {story}",
        )),
        rows,
        decision_counter,
        category_counter,
        facet_counter,
        pending_count: pending_rows.len(),
        pending_rows,
        pending_actions,
        global_feedback_backlog: build_feedback_backlog(&feedback_history),
    })
}

/// `build_story_conflict_snapshot`：`build_story_conflict_snapshot_from_path` 的
/// 缺省 `limit=200` 薄封装（供 reports 侧消费）。
pub fn build_story_conflict_snapshot(draft_path: &Path) -> Result<StoryConflictSnapshot> {
    build_story_conflict_snapshot_from_path(draft_path, 200)
}

fn unavailable_snapshot(reason: &str) -> StoryConflictSnapshot {
    StoryConflictSnapshot {
        available: false,
        reason: Some(reason.to_string()),
        novel_dir: None,
        db_path: None,
        feedback_path: None,
        story: None,
        review_queue_command: None,
        feedback_summary_command: None,
        rows: Vec::new(),
        decision_counter: Ctr::default(),
        category_counter: Ctr::default(),
        facet_counter: Ctr::default(),
        pending_rows: Vec::new(),
        pending_count: 0,
        pending_actions: Vec::new(),
        global_feedback_backlog: Vec::new(),
    }
}
