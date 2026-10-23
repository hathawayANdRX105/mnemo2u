//! 卡片 / 文档扫描与索引构建（SQLite + FTS5）。

use super::*;
// ---------------------------------------------------------------------------
// 卡片 / 文档扫描
// ---------------------------------------------------------------------------

/// `split_fact_segments`。
pub fn split_fact_segments(text: &str) -> Vec<String> {
    FACT_SEGMENT_SPLIT_RE
        .split(text)
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect()
}

/// `is_negated_relationship_segment`。
pub fn is_negated_relationship_segment(segment: &str, term: &str) -> bool {
    if !["信任", "默认", "配合", "护住", "并肩", "愿意跟", "接住"].contains(&term) {
        return false;
    }
    NEGATED_RELATION_PATTERNS
        .iter()
        .any(|pattern| pattern.is_match(segment))
}

/// `collect_local_fact_cues`。
pub fn collect_local_fact_cues(
    passage_text: &str,
    entity_name: &str,
    fact_type: &str,
    terms: &[&str],
) -> Vec<String> {
    let mut cues: Vec<String> = Vec::new();
    for segment in split_fact_segments(passage_text) {
        if !segment.contains(entity_name) {
            continue;
        }
        let entity_index = segment.find(entity_name).unwrap_or(0);
        for &term in terms {
            let Some(term_index) = segment.find(term) else {
                continue;
            };
            if (term_index as i64 - entity_index as i64).abs() > 60 {
                continue;
            }
            if fact_type == "relationship_close" && is_negated_relationship_segment(&segment, term)
            {
                continue;
            }
            if !cues.iter().any(|c| c == term) {
                cues.push(term.to_string());
            }
        }
    }
    cues
}

/// `parse_field_map`。
pub fn parse_field_map(text: &str) -> BTreeMap<String, String> {
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for caps in FIELD_RE.captures_iter(text) {
        let key = normalize_whitespace(caps.get(1).unwrap().as_str());
        let value = normalize_whitespace(caps.get(2).unwrap().as_str());
        fields.insert(key, value);
    }
    fields
}

/// `infer_title_variants`。
pub fn infer_title_variants(title: &str) -> Vec<String> {
    let mut variants: Vec<String> = Vec::new();
    if title.contains('·') {
        for part in title.split('·') {
            let cleaned = normalize_whitespace(part);
            if !cleaned.is_empty() && !variants.iter().any(|v| v == &cleaned) {
                variants.push(cleaned);
            }
        }
    }
    variants
}

/// `extract_names`。
pub fn extract_names(title: &str, fields: &BTreeMap<String, String>) -> Vec<String> {
    let mut names: Vec<String> = vec![title.trim().to_string()];
    for variant in infer_title_variants(title) {
        if !names.iter().any(|n| n == &variant) {
            names.push(variant);
        }
    }
    let alias_value = fields.get("别名 / 英文名").cloned().unwrap_or_default();
    if !alias_value.is_empty() && !["无", "-", "待定"].contains(&alias_value.as_str()) {
        for part in ALIASES_SPLIT_RE.split(&alias_value) {
            let cleaned = normalize_whitespace(part);
            if !cleaned.is_empty() && !names.iter().any(|n| n == &cleaned) {
                names.push(cleaned);
            }
        }
    }
    names
}

fn iter_glob(novel_dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let full = novel_dir.join(pattern).to_string_lossy().into_owned();
    let mut paths = std::collections::BTreeSet::new();
    for entry in glob::glob(&full)?.filter_map(Result::ok) {
        paths.insert(entry);
    }
    let mut out: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_file()).collect();
    out.sort();
    Ok(out)
}

/// `load_entities`。
pub fn load_entities(novel_dir: &Path, paths: &crate::config::PathsConfig) -> Result<Vec<Entity>> {
    let mut entities: Vec<Entity> = Vec::new();
    for card_path in iter_glob(novel_dir, &paths.cards_glob())? {
        if card_path
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new("_templates"))
        {
            continue;
        }
        let text = std::fs::read_to_string(&card_path)?;
        let Some(matched) = HEADER_RE.captures(&text) else {
            continue;
        };
        let title = normalize_whitespace(matched.get(1).unwrap().as_str());
        let fields = parse_field_map(&text);
        let category = card_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let card_id = fields.get("卡片 ID").cloned().unwrap_or_default();
        let names = extract_names(&title, &fields);
        entities.push(Entity {
            category,
            card_path,
            title,
            card_id,
            names,
        });
    }
    Ok(entities)
}

/// `iter_documents`。
pub fn iter_documents(
    novel_dir: &Path,
    paths: &crate::config::PathsConfig,
) -> Result<Vec<PathBuf>> {
    let mut docs = std::collections::BTreeSet::new();
    for pattern in paths.doc_globs() {
        for path in iter_glob(novel_dir, &pattern)? {
            docs.insert(path);
        }
    }
    let mut out: Vec<PathBuf> = docs.into_iter().collect();
    out.sort();
    Ok(out)
}

/// 按换行迭代（`\r\n` 计一次换行）。
fn splitlines_iter(text: &str) -> impl Iterator<Item = &str> {
    struct Splitlines<'a>(&'a str, usize, usize);
    impl<'a> Iterator for Splitlines<'a> {
        type Item = &'a str;
        fn next(&mut self) -> Option<&'a str> {
            let (text, pos, len) = (self.0, self.1, self.2);
            if pos >= len {
                return None;
            }
            let mut i = pos;
            while i < len {
                let (ch, width) = text[i..].chars().next().map(|c| (c, c.len_utf8())).unwrap();
                let line = &text[pos..i];
                match ch {
                    '\r' | '\n' => {
                        i += width;
                        if ch == '\r' && i < len && text[i..].starts_with('\n') {
                            i += 1;
                        }
                        self.1 = i;
                        return Some(line);
                    }
                    '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}'
                    | '\u{2028}' | '\u{2029}' => {
                        i += width;
                        self.1 = i;
                        return Some(line);
                    }
                    _ => i += width,
                }
            }
            let line = &text[pos..len];
            self.1 = len;
            if line.is_empty() {
                None
            } else {
                Some(line)
            }
        }
    }
    Splitlines(text, 0, text.len())
}

/// `split_passages`：段落 → `(line_start, line_end, text)`。
pub fn split_passages(text: &str) -> Vec<(usize, usize, String)> {
    let mut passages: Vec<(usize, usize, String)> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut start_line = 1usize;
    let mut current_line = 1usize;
    for raw_line in splitlines_iter(text) {
        let line = raw_line.trim_end();
        if !line.trim().is_empty() {
            if paragraph.is_empty() {
                start_line = current_line;
            }
            paragraph.push(line.to_string());
        } else if !paragraph.is_empty() {
            let end = current_line - 1;
            passages.push((start_line, end, paragraph.join("\n")));
            paragraph.clear();
        }
        current_line += 1;
    }
    if !paragraph.is_empty() {
        passages.push((start_line, current_line - 1, paragraph.join("\n")));
    }
    passages
}

/// schema DDL：`sqlite_master.sql` 存执行文本原样（dump 字节级一致要求 8 空格缩进）。
const SCHEMA_DDL: &str = r#"
        DROP TABLE IF EXISTS fact_candidates;
        DROP TABLE IF EXISTS mentions;
        DROP TABLE IF EXISTS passages;
        DROP TABLE IF EXISTS entity_names;
        DROP TABLE IF EXISTS entities;
        DROP TABLE IF EXISTS documents;
        DROP TABLE IF EXISTS passage_fts;

        CREATE TABLE documents (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE,
            doc_type TEXT NOT NULL,
            arc TEXT,
            story TEXT,
            chapter TEXT
        );

        CREATE TABLE passages (
            id INTEGER PRIMARY KEY,
            document_id INTEGER NOT NULL,
            line_start INTEGER NOT NULL,
            line_end INTEGER NOT NULL,
            text TEXT NOT NULL,
            FOREIGN KEY(document_id) REFERENCES documents(id)
        );

        CREATE VIRTUAL TABLE passage_fts USING fts5(
            path UNINDEXED,
            text,
            content=''
        );

        CREATE TABLE entities (
            id INTEGER PRIMARY KEY,
            category TEXT NOT NULL,
            card_path TEXT NOT NULL,
            card_id TEXT,
            title TEXT NOT NULL
        );

        CREATE TABLE entity_names (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id)
        );

        CREATE TABLE mentions (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            entity_name_id INTEGER NOT NULL,
            document_id INTEGER NOT NULL,
            passage_id INTEGER NOT NULL,
            count INTEGER NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            FOREIGN KEY(entity_name_id) REFERENCES entity_names(id),
            FOREIGN KEY(document_id) REFERENCES documents(id),
            FOREIGN KEY(passage_id) REFERENCES passages(id)
        );

        CREATE TABLE fact_candidates (
            id INTEGER PRIMARY KEY,
            entity_id INTEGER NOT NULL,
            entity_name_id INTEGER NOT NULL,
            document_id INTEGER NOT NULL,
            passage_id INTEGER NOT NULL,
            fact_type TEXT NOT NULL,
            cue TEXT NOT NULL,
            FOREIGN KEY(entity_id) REFERENCES entities(id),
            FOREIGN KEY(entity_name_id) REFERENCES entity_names(id),
            FOREIGN KEY(document_id) REFERENCES documents(id),
            FOREIGN KEY(passage_id) REFERENCES passages(id)
        );

        CREATE INDEX idx_passages_document ON passages(document_id);
        CREATE INDEX idx_mentions_entity ON mentions(entity_id);
        CREATE INDEX idx_mentions_document ON mentions(document_id);
        CREATE INDEX idx_entity_names_name ON entity_names(name);
        CREATE INDEX idx_fact_candidates_entity ON fact_candidates(entity_id);
        CREATE INDEX idx_fact_candidates_type ON fact_candidates(fact_type);
        "#;

fn build_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_DDL)?;
    Ok(())
}

/// 全匹配语义（regex 1.x 无 `is_exact_match`，用 `find` + 全跨度判断）。
fn part_full_match(re: &Regex, part: &str) -> bool {
    re.find(part)
        .is_some_and(|m| m.start() == 0 && m.end() == part.len())
}

/// `classify_document`：`(doc_type, arc, story, chapter)`。
pub fn classify_document(
    path: &Path,
    novel_dir: &Path,
) -> (String, Option<String>, Option<String>, Option<String>) {
    let relative = path.strip_prefix(novel_dir).unwrap_or(path);
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let doc_type = parts.first().cloned().unwrap_or_default();
    let mut arc: Option<String> = None;
    for part in &parts {
        if part_full_match(&ARC_PART_RE, part) {
            arc = Some(part.to_lowercase());
            break;
        }
    }
    let mut story: Option<String> = None;
    let mut chapter: Option<String> = None;
    for part in &parts {
        if part_full_match(&STORY_PART_RE, part) || part_full_match(&INTERLUDE_PART_RE, part) {
            story = Some(part.to_lowercase());
        }
        if part_full_match(&CHAPTER_PART_RE, part) {
            chapter = Some(part[..part.len() - 3].to_string());
        }
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned().to_lowercase())
        .unwrap_or_default();
    if story.is_none() {
        if let Some(caps) = STORY_CHAPTER_FILE_RE.captures(&stem) {
            story = Some(caps.get(1).unwrap().as_str().to_lowercase());
        } else if let Some(caps) = STORY_PLAN_FILE_RE.captures(&stem) {
            story = Some(format!(
                "story{}",
                caps.get(2).unwrap().as_str().to_lowercase()
            ));
        } else if let Some(caps) = INTERLUDE_PLAN_FILE_RE.captures(&stem) {
            story = Some(format!(
                "interlude{}",
                caps.get(2).unwrap().as_str().to_lowercase()
            ));
        } else if doc_type == "chapter-plan"
            && path
                .file_name()
                .map(|n| CHAPTER_ONLY_FILE_RE.is_match(&n.to_string_lossy()))
                .unwrap_or(false)
        {
            story = Some("story1".to_string());
        }
    }
    (doc_type, arc, story, chapter)
}

/// `build_index`：重建索引（实体插入后与函数尾部各提交一次；
/// rusqlite autocommit 逐语句提交，终态一致）。
pub fn build_index(
    novel_dir: &Path,
    db_path: &Path,
    paths: &crate::config::PathsConfig,
) -> Result<()> {
    let entities = load_entities(novel_dir, paths)?;
    let documents = iter_documents(novel_dir, paths)?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(db_path)?;
    build_schema(&conn)?;

    for entity in &entities {
        conn.execute(
            "INSERT INTO entities(category, card_path, card_id, title) VALUES (?1, ?2, ?3, ?4)",
            params![
                entity.category,
                entity.card_path.display().to_string(),
                entity.card_id,
                entity.title
            ],
        )?;
        let entity_id = conn.last_insert_rowid();
        for name in &entity.names {
            conn.execute(
                "INSERT INTO entity_names(entity_id, name) VALUES (?1, ?2)",
                params![entity_id, name],
            )?;
        }
    }

    let name_records: Vec<(i64, i64, String)> = {
        let mut stmt = conn
            .prepare("SELECT id, entity_id, name FROM entity_names ORDER BY LENGTH(name) DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for path in &documents {
        let text = std::fs::read_to_string(path)?;
        let (doc_type, arc, story, chapter) = classify_document(path, novel_dir);
        conn.execute(
            "INSERT INTO documents(path, doc_type, arc, story, chapter) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                path.display().to_string(),
                doc_type,
                arc,
                story,
                chapter
            ],
        )?;
        let document_id = conn.last_insert_rowid();
        for (line_start, line_end, passage_text) in split_passages(&text) {
            conn.execute(
                "INSERT INTO passages(document_id, line_start, line_end, text) VALUES (?1, ?2, ?3, ?4)",
                params![document_id, line_start as i64, line_end as i64, passage_text],
            )?;
            let passage_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO passage_fts(rowid, path, text) VALUES (?1, ?2, ?3)",
                params![passage_id, path.display().to_string(), passage_text],
            )?;
            for record in &name_records {
                let (entity_name_id, entity_id, name) = record;
                let count = passage_text.matches(name.as_str()).count();
                if count == 0 {
                    continue;
                }
                conn.execute(
                    "INSERT INTO mentions(entity_id, entity_name_id, document_id, passage_id, count)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![entity_id, entity_name_id, document_id, passage_id, count],
                )?;
                for (fact_type, terms) in FACT_TERM_GROUPS {
                    let cues = collect_local_fact_cues(&passage_text, name, fact_type, terms);
                    for cue in &cues {
                        conn.execute(
                            "INSERT INTO fact_candidates(entity_id, entity_name_id, document_id, passage_id, fact_type, cue)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![entity_id, entity_name_id, document_id, passage_id, fact_type, cue],
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// `open_db`：不存在则创建（`sqlite3` connect 语义）。
pub fn open_db(path: &Path) -> Result<Connection> {
    Ok(Connection::open(path)?)
}

/// `resolve_db_path`：db 文件 / consistency 目录 / novel 目录三形态。
pub fn resolve_db_path(raw_path: &Path) -> PathBuf {
    if raw_path.is_dir() {
        if raw_path.file_name().and_then(|n| n.to_str()) == Some("consistency") {
            return raw_path.join("consistency.sqlite3");
        }
        if let Some(novel_dir) = find_novel_dir(raw_path) {
            return novel_dir
                .join("research")
                .join("consistency")
                .join("consistency.sqlite3");
        }
    }
    if let Some(name) = raw_path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with("novel") && raw_path.extension().is_none() {
            return raw_path
                .join("research")
                .join("consistency")
                .join("consistency.sqlite3");
        }
    }
    raw_path.to_path_buf()
}
