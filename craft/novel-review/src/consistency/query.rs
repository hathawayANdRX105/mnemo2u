//! 张力 / 关系 / 对齐 / 证据查询（SQL 参数化，原文照抄）。

use super::*;
// ---------------------------------------------------------------------------
// 查询（SQL 原文照抄，参数化）
// ---------------------------------------------------------------------------

/// `query_story_tension_rows`。
pub fn query_story_tension_rows(conn: &Connection, limit: i64) -> Result<Vec<TensionRow>> {
    let sql = "
        SELECT
            d.story,
            e.title,
            e.category,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'injury_negative' THEN f.cue END) AS injury_negative,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'injury_stable' THEN f.cue END) AS injury_stable,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'equipment_damaged' THEN f.cue END) AS equipment_damaged,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'equipment_active' THEN f.cue END) AS equipment_active
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        WHERE d.story IS NOT NULL
          AND e.category IN ('characters', 'units', 'items', 'technology')
        GROUP BY d.story, e.id
        HAVING
            (injury_negative IS NOT NULL AND injury_stable IS NOT NULL)
            OR
            (equipment_damaged IS NOT NULL AND equipment_active IS NOT NULL)
        ORDER BY d.story, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(TensionRow {
            story: row.get::<_, String>(0)?,
            title: row.get(1)?,
            category: row.get(2)?,
            injury_negative: row.get(3)?,
            injury_stable: row.get(4)?,
            equipment_damaged: row.get(5)?,
            equipment_active: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_goal_tension_rows`。
pub fn query_story_goal_tension_rows(conn: &Connection, limit: i64) -> Result<Vec<GoalTensionRow>> {
    let sql = "
        SELECT
            d.story,
            e.title,
            e.category,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_assigned' THEN f.cue END) AS goal_assigned,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_changed' THEN f.cue END) AS goal_changed,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_completed' THEN f.cue END) AS goal_completed
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        WHERE d.story IS NOT NULL
          AND e.category IN ('characters', 'units', 'organizations')
        GROUP BY d.story, e.id
        HAVING
            (goal_assigned IS NOT NULL AND goal_changed IS NOT NULL)
            OR
            (goal_assigned IS NOT NULL AND goal_completed IS NOT NULL)
        ORDER BY d.story, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(GoalTensionRow {
            story: row.get::<_, String>(0)?,
            title: row.get(1)?,
            category: row.get(2)?,
            goal_assigned: row.get(3)?,
            goal_changed: row.get(4)?,
            goal_completed: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_relationship_tension_rows`。
pub fn query_story_relationship_tension_rows(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<RelationshipTensionRow>> {
    let sql = "
        SELECT
            d.story,
            e.title,
            e.category,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'relationship_close' THEN f.cue END) AS relationship_close,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'relationship_distant' THEN f.cue END) AS relationship_distant
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        WHERE d.story IS NOT NULL
          AND e.category IN ('characters', 'units', 'organizations')
        GROUP BY d.story, e.id
        HAVING relationship_close IS NOT NULL AND relationship_distant IS NOT NULL
        ORDER BY d.story, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(RelationshipTensionRow {
            story: row.get::<_, String>(0)?,
            title: row.get(1)?,
            category: row.get(2)?,
            relationship_close: row.get(3)?,
            relationship_distant: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `collect_relationship_cues`。
pub fn collect_relationship_cues(text: &str) -> (Vec<&'static str>, Vec<&'static str>) {
    let close = RELATIONSHIP_CLOSE_TERMS
        .iter()
        .copied()
        .filter(|t| text.contains(t))
        .collect();
    let distant = RELATIONSHIP_DISTANT_TERMS
        .iter()
        .copied()
        .filter(|t| text.contains(t))
        .collect();
    (close, distant)
}

/// `has_relationship_pronoun_bridge`。
pub fn has_relationship_pronoun_bridge(text: &str) -> bool {
    ["他", "她", "你", "你们", "两人", "两个人", "对方"]
        .iter()
        .any(|token| text.contains(token))
}

/// `query_story_relationship_pair_rows`。
pub fn query_story_relationship_pair_rows(conn: &Connection, limit: i64) -> Result<Vec<PairRow>> {
    let sql = "
        SELECT
            d.story,
            d.path,
            d.chapter,
            p.line_start,
            p.line_end,
            p.text,
            e.title,
            n.name AS matched_name
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        JOIN passages p ON p.id = m.passage_id
        WHERE d.story IS NOT NULL
          AND d.doc_type IN ('story-plan', 'chapter-plan', 'drafts')
          AND e.category = 'characters'
        ORDER BY d.story, d.path, p.line_start, e.title, matched_name";
    let mut stmt = conn.prepare(sql)?;
    type RawPairRow = (
        String,
        String,
        Option<String>,
        i64,
        i64,
        String,
        String,
        String,
    );
    let raw_rows: Vec<RawPairRow> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    type PassageKey = (String, String, Option<String>, i64, i64, String);
    // (story, path, chapter, line_start, line_end, text) → {title: [matched names]}（首现序）
    let mut by_passage: BTreeMap<PassageKey, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for (story, path, chapter, line_start, line_end, text, title, matched_name) in raw_rows {
        let key = (story, path, chapter, line_start, line_end, text);
        let entry = by_passage.entry(key).or_default();
        if let Some(names) = entry.iter_mut().find(|item| item.0 == title) {
            if !names.1.contains(&matched_name) {
                names.1.push(matched_name);
            }
        } else {
            entry.push((title, vec![matched_name]));
        }
    }

    let mut pair_rows: Vec<PairRow> = Vec::new();
    for ((story, path, chapter, line_start, line_end, passage_text), title_map) in
        by_passage.into_iter()
    {
        if title_map.len() < 2 {
            continue;
        }
        let mut all_titles: Vec<String> = title_map.iter().map(|(t, _)| t.clone()).collect();
        all_titles.sort();
        all_titles.dedup();
        let segments = {
            let split = split_fact_segments(&passage_text);
            if split.is_empty() {
                vec![passage_text.clone()]
            } else {
                split
            }
        };
        let mut seen_pairs: std::collections::BTreeSet<(String, String, String, String)> =
            std::collections::BTreeSet::new();
        for segment in &segments {
            let (close_cues, distant_cues) = collect_relationship_cues(segment);
            if close_cues.is_empty() && distant_cues.is_empty() {
                continue;
            }
            let mut present_titles: Vec<String> = Vec::new();
            for (title, names) in &title_map {
                if names.iter().any(|name| segment.contains(name)) {
                    present_titles.push(title.clone());
                }
            }
            if present_titles.len() < 2 {
                if all_titles.len() == 2 && has_relationship_pronoun_bridge(segment) {
                    present_titles = all_titles.clone();
                } else {
                    continue;
                }
            }
            if present_titles.len() < 2 {
                continue;
            }
            let mut ordered_titles = present_titles;
            ordered_titles.sort();
            ordered_titles.dedup();
            for (idx, left_title) in ordered_titles.iter().enumerate() {
                for right_title in ordered_titles.iter().skip(idx + 1) {
                    let pair_key = (
                        story.clone(),
                        left_title.clone(),
                        right_title.clone(),
                        segment.clone(),
                    );
                    if !seen_pairs.insert(pair_key.clone()) {
                        continue;
                    }
                    pair_rows.push(PairRow {
                        story: story.clone(),
                        path: path.clone(),
                        chapter: chapter.clone(),
                        line_start,
                        line_end,
                        text: segment.clone(),
                        left_title: left_title.clone(),
                        right_title: right_title.clone(),
                        close_cues: close_cues.join(","),
                        distant_cues: distant_cues.join(","),
                    });
                    if pair_rows.len() as i64 >= limit {
                        return Ok(pair_rows);
                    }
                }
            }
        }
    }
    Ok(pair_rows)
}

/// `query_story_alignment_rows`。
pub fn query_story_alignment_rows(conn: &Connection, limit: i64) -> Result<Vec<AlignmentRow>> {
    let sql = "
        WITH per_story AS (
            SELECT
                e.title AS entity_title,
                d.story AS story,
                MAX(CASE WHEN d.doc_type IN ('story-plan', 'chapter-plan') THEN 1 ELSE 0 END) AS in_plan,
                MAX(CASE WHEN d.doc_type = 'drafts' THEN 1 ELSE 0 END) AS in_draft
            FROM mentions m
            JOIN entities e ON e.id = m.entity_id
            JOIN documents d ON d.id = m.document_id
            WHERE d.story IS NOT NULL
            GROUP BY e.id, d.story
        )
        SELECT
            story,
            SUM(CASE WHEN in_plan = 1 THEN 1 ELSE 0 END) AS plan_entities,
            SUM(CASE WHEN in_draft = 1 THEN 1 ELSE 0 END) AS draft_entities,
            GROUP_CONCAT(CASE WHEN in_plan = 1 AND in_draft = 0 THEN entity_title END, ' | ') AS plan_only_entities,
            GROUP_CONCAT(CASE WHEN in_plan = 0 AND in_draft = 1 THEN entity_title END, ' | ') AS draft_only_entities
        FROM per_story
        GROUP BY story
        ORDER BY story
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(AlignmentRow {
            story: row.get::<_, String>(0)?,
            plan_entities: row.get(1)?,
            draft_entities: row.get(2)?,
            plan_only_entities: row.get(3)?,
            draft_only_entities: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_alignment_gap_rows`。
pub fn query_story_alignment_gap_rows(conn: &Connection, limit: i64) -> Result<Vec<AlignmentRow>> {
    let rows = query_story_alignment_rows(conn, limit)?;
    Ok(rows
        .into_iter()
        .filter(|row| row.plan_only_entities.is_some() || row.draft_only_entities.is_some())
        .collect())
}

/// `query_story_alignment_evidence`。
pub fn query_story_alignment_evidence(
    conn: &Connection,
    story: &str,
    entity_title: &str,
    side: &str,
    limit: i64,
) -> Result<Vec<EvidenceRow>> {
    let doc_types: &[&str] = match side {
        "plan" => &["story-plan", "chapter-plan"],
        "draft" => &["drafts"],
        other => anyhow::bail!("Unknown side: {other}"),
    };
    let placeholders = vec!["?"; doc_types.len()].join(", ");
    let sql = format!(
        r#"
        SELECT d.path, p.line_start, p.line_end, p.text
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN documents d ON d.id = m.document_id
        JOIN passages p ON p.id = m.passage_id
        WHERE d.story = ? AND e.title = ? AND d.doc_type IN ({placeholders})
        ORDER BY d.path, p.line_start
        LIMIT ?
    "#
    );
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(story.to_string()) as Box<dyn ToSql>,
        Box::new(entity_title.to_string()) as Box<dyn ToSql>,
    ];
    for dt in doc_types {
        params.push(Box::new(dt.to_string()) as Box<dyn ToSql>);
    }
    params.push(Box::new(limit) as Box<dyn ToSql>);
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(EvidenceRow {
            path: row.get::<_, String>(0)?,
            line_start: row.get(1)?,
            line_end: row.get(2)?,
            text: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_tension_evidence`。
pub fn query_story_tension_evidence(
    conn: &Connection,
    story: &str,
    entity_title: &str,
    limit: i64,
    fact_types: Option<&[String]>,
) -> Result<Vec<FactEvidenceRow>> {
    let mut sql = String::from(
        "
        SELECT
            d.path,
            p.line_start,
            p.line_end,
            p.text,
            f.fact_type,
            f.cue
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        JOIN passages p ON p.id = f.passage_id
        WHERE d.story = ? AND e.title = ?",
    );
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(story.to_string()) as Box<dyn ToSql>,
        Box::new(entity_title.to_string()) as Box<dyn ToSql>,
    ];
    if let Some(types) = fact_types.filter(|t| !t.is_empty()) {
        let placeholders = vec!["?"; types.len()].join(", ");
        sql.push_str(&format!(" AND f.fact_type IN ({placeholders})"));
        for t in types {
            params.push(Box::new(t.clone()) as Box<dyn ToSql>);
        }
    }
    sql.push_str(
        "
        ORDER BY d.path, p.line_start
        LIMIT ?
    ",
    );
    params.push(Box::new(limit) as Box<dyn ToSql>);
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(sql.as_str())?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(FactEvidenceRow {
            path: row.get::<_, String>(0)?,
            line_start: row.get(1)?,
            line_end: row.get(2)?,
            text: row.get(3)?,
            fact_type: row.get(4)?,
            cue: row.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_alias_drift_rows`。
pub fn query_story_alias_drift_rows(conn: &Connection, limit: i64) -> Result<Vec<AliasDriftRow>> {
    let sql = "
        SELECT
            d.story,
            e.title,
            e.category,
            GROUP_CONCAT(
                DISTINCT CASE WHEN d.doc_type = 'drafts' THEN n.name END
            ) AS draft_aliases,
            GROUP_CONCAT(
                DISTINCT CASE WHEN d.doc_type IN ('story-plan', 'chapter-plan') THEN n.name END
            ) AS plan_aliases,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN n.name END) AS draft_alias_count,
            COUNT(DISTINCT CASE WHEN d.doc_type IN ('story-plan', 'chapter-plan') THEN n.name END) AS plan_alias_count
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        WHERE d.story IS NOT NULL
        GROUP BY d.story, e.id
        HAVING draft_alias_count >= 1 AND plan_alias_count >= 1 AND draft_aliases != plan_aliases
        ORDER BY d.story, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(AliasDriftRow {
            story: row.get::<_, String>(0)?,
            title: row.get(1)?,
            category: row.get(2)?,
            draft_aliases: row.get(3)?,
            plan_aliases: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_story_alias_evidence`。
pub fn query_story_alias_evidence(
    conn: &Connection,
    story: &str,
    entity_title: &str,
    side: &str,
    limit: i64,
) -> Result<Vec<AliasEvidenceRow>> {
    let doc_types: &[&str] = match side {
        "plan" => &["story-plan", "chapter-plan"],
        "draft" => &["drafts"],
        other => anyhow::bail!("Unknown side: {other}"),
    };
    let placeholders = vec!["?"; doc_types.len()].join(", ");
    let sql = format!(
        r#"
        SELECT
            d.path,
            p.line_start,
            p.line_end,
            p.text,
            n.name AS matched_name
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        JOIN passages p ON p.id = m.passage_id
        WHERE d.story = ? AND e.title = ? AND d.doc_type IN ({placeholders})
        ORDER BY d.path, p.line_start
        LIMIT ?
    "#
    );
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(story.to_string()) as Box<dyn ToSql>,
        Box::new(entity_title.to_string()) as Box<dyn ToSql>,
    ];
    for dt in doc_types {
        params.push(Box::new(dt.to_string()) as Box<dyn ToSql>);
    }
    params.push(Box::new(limit) as Box<dyn ToSql>);
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(sql.as_str())?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok(AliasEvidenceRow {
            path: row.get::<_, String>(0)?,
            line_start: row.get(1)?,
            line_end: row.get(2)?,
            text: row.get(3)?,
            matched_name: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `query_fact_support_summary`。
pub fn query_fact_support_summary(
    conn: &Connection,
    story: &str,
    entity_title: &str,
    fact_types: &[String],
) -> Result<(i64, i64, i64)> {
    let placeholders = vec!["?"; fact_types.len()].join(", ");
    let sql = format!(
        r#"
        SELECT
            COUNT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN d.id END) AS draft_docs,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'chapter-plan' THEN d.id END) AS chapter_plan_docs,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'story-plan' THEN d.id END) AS story_plan_docs
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        WHERE d.story = ? AND e.title = ? AND f.fact_type IN ({placeholders})
    "#
    );
    let mut params: Vec<Box<dyn ToSql>> = vec![
        Box::new(story.to_string()) as Box<dyn ToSql>,
        Box::new(entity_title.to_string()) as Box<dyn ToSql>,
    ];
    for t in fact_types {
        params.push(Box::new(t.clone()) as Box<dyn ToSql>);
    }
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(sql.as_str())?;
    let row = stmt
        .query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .next()
        .ok_or_else(|| anyhow::anyhow!("support 查询无结果"))??;
    // COUNT 非 NULL，直接返回。
    Ok(row)
}

/// `score_fact_confidence`。
pub fn score_fact_confidence(support: &(i64, i64, i64)) -> (String, String) {
    let (draft_docs, chapter_plan_docs, story_plan_docs) = *support;
    if draft_docs >= 2 {
        return ("high".into(), format!("draft_docs={draft_docs}"));
    }
    if draft_docs >= 1 && (chapter_plan_docs + story_plan_docs) >= 1 {
        return (
            "high".into(),
            format!(
                "draft_docs={draft_docs} upstream_docs={}",
                chapter_plan_docs + story_plan_docs
            ),
        );
    }
    if draft_docs >= 1 {
        return ("medium".into(), format!("draft_docs={draft_docs}"));
    }
    if chapter_plan_docs >= 1 && story_plan_docs >= 1 {
        return (
            "medium".into(),
            format!("chapter_plan_docs={chapter_plan_docs} story_plan_docs={story_plan_docs}"),
        );
    }
    (
        "low".into(),
        format!("chapter_plan_docs={chapter_plan_docs} story_plan_docs={story_plan_docs}"),
    )
}

/// `score_alignment_confidence`。
pub fn score_alignment_confidence(summary: &str) -> (String, String) {
    let plan_only = if summary.contains("plan_only=") {
        ALIGN_PLAN_ONLY_RE
            .captures(summary)
            .map(|caps| split_pipe_values(Some(caps.get(1).unwrap().as_str())))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let draft_only = if summary.contains("draft_only=") {
        ALIGN_DRAFT_ONLY_RE
            .captures(summary)
            .map(|caps| split_pipe_values(Some(caps.get(1).unwrap().as_str())))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let entity_count = plan_only.len() + draft_only.len();
    let label = if entity_count >= 3 {
        "high"
    } else if entity_count >= 2 {
        "medium"
    } else {
        "low"
    };
    (label.into(), format!("drift_entities={entity_count}"))
}

/// `score_alias_confidence`。
pub fn score_alias_confidence(plan_aliases: &str, draft_aliases: &str) -> (String, String) {
    let plan_count = split_pipe_values_str(&plan_aliases.replace(',', "|")).len();
    let draft_count = split_pipe_values_str(&draft_aliases.replace(',', "|")).len();
    let label = if plan_count >= 2 && draft_count >= 2 {
        "high"
    } else if plan_count >= 1 && draft_count >= 1 {
        "medium"
    } else {
        "low"
    };
    (
        label.into(),
        format!("plan_aliases={plan_count} draft_aliases={draft_count}"),
    )
}
