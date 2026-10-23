//! stdout 打印（逐行固定）。

use super::*;
// ---------------------------------------------------------------------------
// 打印（stdout 逐行固定）
// ---------------------------------------------------------------------------

/// `print_search_results`。
pub fn print_search_results(conn: &Connection, term: &str, limit: i64) -> Result<()> {
    let sql = "
        SELECT d.path, ps.line_start, ps.line_end, ps.text
        FROM passage_fts f
        JOIN passages ps ON ps.id = f.rowid
        JOIN documents d ON d.id = ps.document_id
        WHERE passage_fts MATCH ?
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![term, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        println!("No matches.");
        return Ok(());
    }
    for (path, line_start, line_end, text) in rows {
        println!("{path}:{line_start}-{line_end}");
        println!("{}", normalize_whitespace(&text));
        println!();
    }
    Ok(())
}

/// `print_entity_results`。
pub fn print_entity_results(conn: &Connection, name: &str, limit: i64) -> Result<()> {
    let sql = "
        SELECT
            e.title,
            e.category,
            n.name AS matched_name,
            d.path,
            p.line_start,
            p.line_end,
            p.text,
            m.count
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        JOIN passages p ON p.id = m.passage_id
        WHERE e.title = ? OR n.name = ?
        ORDER BY d.path, p.line_start
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![name, name, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        println!("No entity matches.");
        return Ok(());
    }
    let (first_title, first_category, _, _, _, _, _, _) = &rows[0];
    println!("Entity: {first_title} ({first_category})");
    println!();
    for (title, category, matched_name, path, line_start, line_end, text, count) in rows {
        println!("{path}:{line_start}-{line_end} matched=`{matched_name}` count={count}");
        let _ = (title, category);
        println!("{}", normalize_whitespace(&text));
        println!();
    }
    Ok(())
}

/// `print_entity_catalog`。
pub fn print_entity_catalog(conn: &Connection, limit: i64) -> Result<()> {
    let sql = "
        SELECT
            e.title,
            e.category,
            COUNT(m.id) AS mentions,
            (
                SELECT GROUP_CONCAT(name, ' | ')
                FROM (
                    SELECT DISTINCT name
                    FROM entity_names
                    WHERE entity_id = e.id
                    ORDER BY name
                )
            ) AS names
        FROM entities e
        LEFT JOIN mentions m ON m.entity_id = e.id
        GROUP BY e.id
        ORDER BY mentions DESC, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (title, category, mentions, names) in rows {
        let names = names.unwrap_or_default();
        println!("{title} [{category}] mentions={mentions} names={names}");
    }
    Ok(())
}

/// `print_entity_facts`。
pub fn print_entity_facts(conn: &Connection, name: &str, limit: i64) -> Result<()> {
    let sql = "
        SELECT
            e.title,
            e.category,
            n.name AS matched_name,
            d.path,
            d.story,
            p.line_start,
            p.line_end,
            p.text,
            f.fact_type,
            f.cue
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN entity_names n ON n.id = f.entity_name_id
        JOIN documents d ON d.id = f.document_id
        JOIN passages p ON p.id = f.passage_id
        WHERE e.title = ? OR n.name = ?
        ORDER BY d.path, p.line_start
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![name, name, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        println!("No fact matches.");
        return Ok(());
    }
    let (first_title, first_category, _, _, _, _, _, _, _, _) = &rows[0];
    println!("Entity Facts: {first_title} ({first_category})");
    println!();
    for (title, category, matched_name, path, story, line_start, line_end, text, fact_type, cue) in
        rows
    {
        let story = story.as_deref().unwrap_or("-");
        let _ = (title, category);
        println!(
            "{path}:{line_start}-{line_end} story=`{story}` matched=`{matched_name}` fact=`{fact_type}` cue=`{cue}`"
        );
        println!("{}", normalize_whitespace(&text));
        println!();
    }
    Ok(())
}

/// `print_story_facts`。
pub fn print_story_facts(conn: &Connection, story: &str, limit: i64) -> Result<()> {
    let sql = "
        SELECT
            e.title,
            e.category,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'injury_negative' THEN f.cue END) AS injury_negative,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'injury_stable' THEN f.cue END) AS injury_stable,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'equipment_damaged' THEN f.cue END) AS equipment_damaged,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'equipment_active' THEN f.cue END) AS equipment_active,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_assigned' THEN f.cue END) AS goal_assigned,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_changed' THEN f.cue END) AS goal_changed,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'goal_completed' THEN f.cue END) AS goal_completed,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'relationship_close' THEN f.cue END) AS relationship_close,
            GROUP_CONCAT(DISTINCT CASE WHEN f.fact_type = 'relationship_distant' THEN f.cue END) AS relationship_distant
        FROM fact_candidates f
        JOIN entities e ON e.id = f.entity_id
        JOIN documents d ON d.id = f.document_id
        WHERE d.story = ?
        GROUP BY e.id
        HAVING
            injury_negative IS NOT NULL
            OR injury_stable IS NOT NULL
            OR equipment_damaged IS NOT NULL
            OR equipment_active IS NOT NULL
            OR goal_assigned IS NOT NULL
            OR goal_changed IS NOT NULL
            OR goal_completed IS NOT NULL
            OR relationship_close IS NOT NULL
            OR relationship_distant IS NOT NULL
        ORDER BY e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(params![story, limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.is_empty() {
        println!("No story fact rows.");
        return Ok(());
    }
    for (
        title,
        category,
        injury_negative,
        injury_stable,
        equipment_damaged,
        equipment_active,
        goal_assigned,
        goal_changed,
        goal_completed,
        relationship_close,
        relationship_distant,
    ) in rows
    {
        let mut details: Vec<String> = Vec::new();
        let append = |out: &mut Vec<String>, value: Option<&str>, name: &str| {
            if let Some(v) = value {
                if !v.is_empty() {
                    out.push(format!("{name}={v}"));
                }
            }
        };
        append(&mut details, injury_negative.as_deref(), "injury_negative");
        append(&mut details, injury_stable.as_deref(), "injury_stable");
        append(
            &mut details,
            equipment_damaged.as_deref(),
            "equipment_damaged",
        );
        append(
            &mut details,
            equipment_active.as_deref(),
            "equipment_active",
        );
        append(&mut details, goal_assigned.as_deref(), "goal_assigned");
        append(&mut details, goal_changed.as_deref(), "goal_changed");
        append(&mut details, goal_completed.as_deref(), "goal_completed");
        append(
            &mut details,
            relationship_close.as_deref(),
            "relationship_close",
        );
        append(
            &mut details,
            relationship_distant.as_deref(),
            "relationship_distant",
        );
        println!("{title} [{category}] :: {}", details.join(" ; "));
    }
    Ok(())
}

/// `print_story_tension`。
pub fn print_story_tension(conn: &Connection, limit: i64) -> Result<()> {
    let rows = query_story_tension_rows(conn, limit)?;
    if rows.is_empty() {
        println!("No story tension rows.");
        return Ok(());
    }
    for row in &rows {
        let mut details: Vec<String> = Vec::new();
        if !row.injury_negative.as_deref().unwrap_or("").is_empty()
            && !row.injury_stable.as_deref().unwrap_or("").is_empty()
        {
            details.push(format!(
                "injury={} -> {}",
                row.injury_negative.as_deref().unwrap_or(""),
                row.injury_stable.as_deref().unwrap_or("")
            ));
        }
        if !row.equipment_damaged.as_deref().unwrap_or("").is_empty()
            && !row.equipment_active.as_deref().unwrap_or("").is_empty()
        {
            details.push(format!(
                "equipment={} -> {}",
                row.equipment_damaged.as_deref().unwrap_or(""),
                row.equipment_active.as_deref().unwrap_or("")
            ));
        }
        println!(
            "{} :: {} [{}] :: {}",
            row.story,
            row.title,
            row.category,
            details.join(" ; ")
        );
    }
    Ok(())
}

/// `print_conflict_evidence`。
pub fn print_conflict_evidence(conn: &Connection, row: &ConflictRow, indent: &str) -> Result<()> {
    match row.evidence_kind {
        EvidenceKind::Fact if row.title != "-" => {
            let fact_types: Option<Vec<String>> =
                (!row.fact_types.is_empty()).then(|| row.fact_types.clone());
            let evidence_rows = query_story_tension_evidence(
                conn,
                &row.story,
                &row.title,
                4,
                fact_types.as_deref(),
            )?;
            for evidence in &evidence_rows {
                println!(
                    "{indent}{}:{}-{} fact=`{}` cue=`{}`",
                    evidence.path,
                    evidence.line_start,
                    evidence.line_end,
                    evidence.fact_type,
                    evidence.cue
                );
                println!("{indent}  {}", normalize_whitespace(&evidence.text));
            }
        }
        EvidenceKind::Alias => {
            for side in ["plan", "draft"] {
                println!("{indent}{side}:");
                let evidence = query_story_alias_evidence(conn, &row.story, &row.title, side, 2)?;
                for e in &evidence {
                    println!(
                        "{indent}  {}:{}-{} matched=`{}`",
                        e.path, e.line_start, e.line_end, e.matched_name
                    );
                    println!("{indent}    {}", normalize_whitespace(&e.text));
                }
            }
        }
        EvidenceKind::Alignment => {
            let summary = row.summary.clone();
            if let Some(plan_match) = ALIGN_PLAN_ONLY_RE.captures(&summary) {
                let titles = split_pipe_values(plan_match.get(1).map(|m| m.as_str()));
                for entity_title in titles.into_iter().take(3) {
                    println!("{indent}plan_only `{entity_title}`");
                    let evidence =
                        query_story_alignment_evidence(conn, &row.story, &entity_title, "plan", 2)?;
                    for e in &evidence {
                        println!("{indent}  {}:{}-{}", e.path, e.line_start, e.line_end);
                        println!("{indent}    {}", normalize_whitespace(&e.text));
                    }
                }
            }
            if let Some(draft_match) = ALIGN_DRAFT_ONLY_RE.captures(&summary) {
                let titles = split_pipe_values(draft_match.get(1).map(|m| m.as_str()));
                for entity_title in titles.into_iter().take(3) {
                    println!("{indent}draft_only `{entity_title}`");
                    let evidence = query_story_alignment_evidence(
                        conn,
                        &row.story,
                        &entity_title,
                        "draft",
                        2,
                    )?;
                    for e in &evidence {
                        println!("{indent}  {}:{}-{}", e.path, e.line_start, e.line_end);
                        println!("{indent}    {}", normalize_whitespace(&e.text));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// `print_conflicts`。
pub fn print_conflicts(conn: &Connection, limit: i64, feedback_path: Option<&Path>) -> Result<()> {
    let conflict_rows = collect_conflict_rows(conn, limit, feedback_path)?;
    if conflict_rows.is_empty() {
        println!("No conflict candidates.");
        return Ok(());
    }
    let mut grouped: BTreeMap<String, Vec<ConflictRow>> = BTreeMap::new();
    for row in conflict_rows {
        grouped.entry(row.category.clone()).or_default().push(row);
    }
    for (category, rows) in grouped {
        println!("## {category}");
        for row in &rows {
            println!(
                "{} :: {} [{}] :: confidence={} support={} :: {}",
                row.story,
                row.title,
                row.entity_category,
                row.confidence,
                row.support_note,
                row.summary
            );
            if !row.feedback_decision.is_empty() {
                let mut extra = format!(" feedback={}", row.feedback_decision);
                if !row.feedback_facet.is_empty() {
                    extra.push_str(&format!(" facet={}", row.feedback_facet));
                }
                if !row.feedback_note.is_empty() {
                    extra.push_str(&format!(" note={}", row.feedback_note));
                }
                println!("  -{extra}");
            }
            print_conflict_evidence(conn, row, "  - ")?;
        }
        println!();
    }
    Ok(())
}

/// `print_review_queue`。
pub fn print_review_queue(
    conn: &Connection,
    feedback_path: &Path,
    story: Option<&str>,
    limit: i64,
) -> Result<()> {
    let summary = summarize_feedback(conn, feedback_path, limit * 4, None)?;
    let unresolved: Vec<&ConflictRow> = summary
        .unresolved
        .iter()
        .filter(|row| story.is_none_or(|s| row.story == s))
        .collect();
    if unresolved.is_empty() {
        println!("No pending review rows.");
        return Ok(());
    }
    println!("## Review Queue");
    match story {
        Some(story) => println!("- story: `{story}`"),
        None => println!("- story: `all`"),
    }
    println!("- feedback_log: `{}`", feedback_path.display());
    println!("- pending_total: `{}`", unresolved.len());
    println!();
    let db_root = parent_or_dot(feedback_path);
    let resolved_db = resolve_db_path(&db_root);
    for row in unresolved.into_iter().take(limit as usize) {
        let command = build_feedback_command(&resolved_db, row, None);
        println!(
            "### {} :: {} :: {} :: confidence={}",
            row.story, row.category, row.title, row.confidence
        );
        println!("- focus: {}", build_pending_review_focus(row));
        println!("- summary: {}", row.summary);
        println!("- command: `{command}`");
        println!("- evidence:");
        print_conflict_evidence(conn, row, "  - ")?;
        println!();
    }
    Ok(())
}

/// `print_feedback_summary`。
pub fn print_feedback_summary(
    conn: &Connection,
    feedback_path: &Path,
    limit: i64,
    story: Option<&str>,
) -> Result<()> {
    let summary = summarize_feedback(conn, feedback_path, limit, story)?;
    let entries = summary.entries;
    let decision_counter = summary.decision_counter;
    let category_counter = summary.category_counter;
    let story_counter = summary.story_counter;
    let facet_counter = summary.facet_counter;
    let unresolved = summary.unresolved;
    let pending_actions = summary.pending_actions;
    let backlog = summary.backlog;
    let _ = summary;

    if entries.is_empty() {
        println!("No feedback entries yet.");
        println!();
    }
    if let Some(story) = &summary.story_filter {
        println!("## Story Filter");
        println!("- story: `{story}`");
        println!();
    }
    println!("## Feedback Decisions");
    for decision in FEEDBACK_DECISIONS {
        println!("- {decision}: {}", decision_counter.get(decision));
    }
    println!();
    println!("## By Category");
    if category_counter.is_empty() {
        println!("- 无");
    } else {
        for (name, count) in category_counter.most_common(12) {
            println!("- {name} x{count}");
        }
    }
    println!();
    println!("## By Facet");
    if facet_counter.is_empty() {
        println!("- 无");
    } else {
        for (name, count) in facet_counter.most_common(12) {
            println!("- {name} x{count}");
        }
    }
    println!();
    println!("## Deposition Suggestions");
    if backlog.is_empty() {
        println!("- 无");
    } else {
        for item in &backlog {
            println!("- `{}` {}", item.target, item.reason);
        }
    }
    println!();
    println!("## By Story");
    if story_counter.is_empty() {
        println!("- 无");
    } else {
        for (name, count) in story_counter.most_common(12) {
            println!("- {name} x{count}");
        }
    }
    println!();
    println!("## Pending Review");
    if unresolved.is_empty() {
        println!("- 无");
    } else {
        for row in unresolved.iter().take(12) {
            println!(
                "- {} :: {} :: {} :: confidence={}",
                row.story, row.category, row.title, row.confidence
            );
        }
    }
    println!();
    println!("## Pending Review Actions");
    if pending_actions.is_empty() {
        println!("- 无");
    } else {
        for item in &pending_actions {
            println!(
                "- {} :: {} :: {} :: confidence={} :: {}",
                item.story, item.category, item.title, item.confidence, item.focus
            );
            println!("  {}", item.command);
        }
    }
    println!();
    Ok(())
}

/// `print_suspects`。
pub fn print_suspects(conn: &Connection, limit: i64) -> Result<()> {
    let sql_alias = "
        SELECT
            e.title,
            e.category,
            d.path,
            COUNT(DISTINCT n.name) AS alias_count,
            GROUP_CONCAT(DISTINCT n.name) AS aliases
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        WHERE d.doc_type != 'concept'
        GROUP BY e.id, d.id
        HAVING alias_count >= 2
        ORDER BY alias_count DESC, d.path, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql_alias)?;
    let alias_rows = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let sql_draft = "
        SELECT
            e.title,
            e.category,
            GROUP_CONCAT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN n.name END) AS draft_aliases,
            GROUP_CONCAT(DISTINCT CASE WHEN d.doc_type IN ('arc-plan', 'story-plan', 'chapter-plan') THEN n.name END) AS upstream_aliases,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN n.name END) AS draft_alias_count,
            COUNT(DISTINCT CASE WHEN d.doc_type IN ('arc-plan', 'story-plan', 'chapter-plan') THEN n.name END) AS upstream_alias_count
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN entity_names n ON n.id = m.entity_name_id
        JOIN documents d ON d.id = m.document_id
        GROUP BY e.id
        HAVING draft_alias_count >= 1 AND upstream_alias_count >= 1 AND draft_aliases != upstream_aliases
        ORDER BY e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql_draft)?;
    let draft_rows = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let sql_plan_only = "
        SELECT
            e.title,
            e.category,
            COUNT(DISTINCT CASE WHEN d.doc_type IN ('arc-plan', 'story-plan', 'chapter-plan') THEN d.id END) AS plan_docs,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN d.id END) AS draft_docs,
            GROUP_CONCAT(DISTINCT CASE WHEN d.doc_type IN ('arc-plan', 'story-plan', 'chapter-plan') THEN d.path END) AS plan_paths
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN documents d ON d.id = m.document_id
        GROUP BY e.id
        HAVING plan_docs >= 2 AND draft_docs = 0
        ORDER BY plan_docs DESC, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql_plan_only)?;
    let plan_only_rows = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let sql_draft_only = "
        SELECT
            e.title,
            e.category,
            COUNT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN d.id END) AS draft_docs,
            COUNT(DISTINCT CASE WHEN d.doc_type IN ('arc-plan', 'story-plan', 'chapter-plan') THEN d.id END) AS plan_docs,
            GROUP_CONCAT(DISTINCT CASE WHEN d.doc_type = 'drafts' THEN d.path END) AS draft_paths
        FROM mentions m
        JOIN entities e ON e.id = m.entity_id
        JOIN documents d ON d.id = m.document_id
        GROUP BY e.id
        HAVING draft_docs >= 2 AND plan_docs = 0
        ORDER BY draft_docs DESC, e.title
        LIMIT ?";
    let mut stmt = conn.prepare(sql_draft_only)?;
    let draft_only_rows = stmt
        .query_map(params![limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let fact_rows = query_story_tension_rows(conn, limit)?;
    let story_alignment_rows = query_story_alignment_gap_rows(conn, limit)?;

    if alias_rows.is_empty()
        && draft_rows.is_empty()
        && plan_only_rows.is_empty()
        && draft_only_rows.is_empty()
        && fact_rows.is_empty()
        && story_alignment_rows.is_empty()
    {
        println!("No suspects.");
        return Ok(());
    }

    if !alias_rows.is_empty() {
        println!("## Same Document Alias Mixing");
        for (title, category, path, aliases) in &alias_rows {
            println!(
                "{path} :: {title} [{category}] aliases={}",
                aliases.as_deref().unwrap_or("")
            );
        }
        println!();
    }
    if !draft_rows.is_empty() {
        println!("## Draft vs Upstream Alias Drift");
        for (title, category, draft_aliases, upstream_aliases) in &draft_rows {
            println!(
                "{title} [{category}] draft={} upstream={}",
                draft_aliases.as_deref().unwrap_or(""),
                upstream_aliases.as_deref().unwrap_or("")
            );
        }
        println!();
    }
    if !plan_only_rows.is_empty() {
        println!("## Plan Mentioned But Draft Missing");
        for (title, category, plan_docs, draft_docs, plan_paths) in &plan_only_rows {
            let paths = plan_paths.as_deref().unwrap_or("");
            let preview = split_pipe_values_str(&paths.replace(',', "|"))
                .into_iter()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ");
            println!(
                "{title} [{category}] plan_docs={plan_docs} draft_docs={draft_docs} paths={preview}"
            );
        }
        println!();
    }
    if !draft_only_rows.is_empty() {
        println!("## Draft Mentioned But Plan Missing");
        for (title, category, draft_docs, plan_docs, draft_paths) in &draft_only_rows {
            let paths = draft_paths.as_deref().unwrap_or("");
            let preview = split_pipe_values_str(&paths.replace(',', "|"))
                .into_iter()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ");
            println!(
                "{title} [{category}] draft_docs={draft_docs} plan_docs={plan_docs} paths={preview}"
            );
        }
        println!();
    }
    if !fact_rows.is_empty() {
        println!("## Draft State Tension Candidates");
        for row in &fact_rows {
            let mut parts: Vec<String> = Vec::new();
            if !row.injury_negative.as_deref().unwrap_or("").is_empty()
                && !row.injury_stable.as_deref().unwrap_or("").is_empty()
            {
                parts.push(format!(
                    "injury={} -> {}",
                    row.injury_negative.as_deref().unwrap_or(""),
                    row.injury_stable.as_deref().unwrap_or("")
                ));
            }
            if !row.equipment_damaged.as_deref().unwrap_or("").is_empty()
                && !row.equipment_active.as_deref().unwrap_or("").is_empty()
            {
                parts.push(format!(
                    "equipment={} -> {}",
                    row.equipment_damaged.as_deref().unwrap_or(""),
                    row.equipment_active.as_deref().unwrap_or("")
                ));
            }
            println!(
                "{} :: {} [{}] {}",
                row.story,
                row.title,
                row.category,
                parts.join(" ; ")
            );
        }
        println!();
    }
    if !story_alignment_rows.is_empty() {
        println!("## Story Plan / Draft Entity Drift");
        for row in &story_alignment_rows {
            let mut details: Vec<String> = Vec::new();
            if let Some(v) = &row.plan_only_entities {
                details.push(format!("plan_only={v}"));
            }
            if let Some(v) = &row.draft_only_entities {
                details.push(format!("draft_only={v}"));
            }
            println!("{} :: {}", row.story, details.join(" ; "));
        }
        println!();
    }
    Ok(())
}

/// `print_story_alignment`。
pub fn print_story_alignment(conn: &Connection, limit: i64) -> Result<()> {
    let rows = query_story_alignment_rows(conn, limit)?;
    if rows.is_empty() {
        println!("No story alignment rows.");
        return Ok(());
    }
    for row in &rows {
        let mut details = vec![
            format!("plan_entities={}", row.plan_entities),
            format!("draft_entities={}", row.draft_entities),
        ];
        if let Some(v) = &row.plan_only_entities {
            details.push(format!("plan_only={v}"));
        }
        if let Some(v) = &row.draft_only_entities {
            details.push(format!("draft_only={v}"));
        }
        println!("{} :: {}", row.story, details.join(" ; "));
    }
    Ok(())
}
