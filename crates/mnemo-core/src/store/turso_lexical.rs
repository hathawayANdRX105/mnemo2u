//! Turso FTS lexical arm for the memory layer (stage doc 03 T3).
//!
//! One plain table `mem_lexical(ws, sess, task, fact_id, text)` primary-keyed
//! per (scope, fact) — so `index` is an `INSERT OR REPLACE` and the same
//! (scope, id) is always one logical row — plus an `USING fts (text)` index
//! (tantivy, behind the local engine's `index_method` experimental feature)
//! that backs `fts_match` / `fts_score`. `scope` is part of the key, so one
//! database file can hold many scopes.
//!
//! `search` applies [`ScopeKey::admits`] *inside its own query* (stage doc 03
//! contract 1): `ws` is always an equality predicate, and `sess` / `task` are
//! added only when the caller constrained them, so a narrow caller never sees
//! another scope's rows — not even their ranks, since the predicates live in
//! the same statement as the FTS scan. Ranking is
//! `fts_score(text, ?) DESC, fact_id ASC`: the FTS score, then the fact id as
//! a deterministic tie-break so equal-scoring rows never reorder between runs.
//!
//! The raw query is sanitized before it reaches the FTS parser: split on
//! whitespace, every token wrapped in double quotes and an inner `"` escaped
//! with a backslash (the FTS phrase syntax of this engine — doubling a quote
//! instead would emit adjacent empty phrases and a `missing space` parse
//! error, so backslash is the only quote escape that keeps the statement
//! valid), joined with an explicit `AND`. The semantics is therefore an
//! AND-of-tokens — every query word must match — and operator-looking user
//! text (`AND`, `OR`, `-`, `"`) degrades to literal phrases instead of
//! becoming FTS operators or a parse error.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turso::{params_from_iter, Builder, Connection, Value as TursoValue};

use crate::core::traits::{LexicalStore, Result, StoreError};
use crate::core::types::{FactId, ScopeKey};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS mem_lexical (
     ws TEXT NOT NULL,
     sess TEXT NOT NULL,
     task TEXT NOT NULL,
     fact_id TEXT NOT NULL,
     text TEXT NOT NULL,
     PRIMARY KEY (ws, sess, task, fact_id)
 );
 CREATE INDEX IF NOT EXISTS mem_lexical_fts ON mem_lexical USING fts (text)";

pub struct TursoLexical {
    _db: Arc<turso::Database>,
    conn: Arc<Mutex<Connection>>,
}

impl TursoLexical {
    /// Open (creating if needed) the database file at `path` and ensure the
    /// lexical table and its FTS index exist. Both statements are idempotent.
    ///
    /// `USING fts` only exists behind the local engine's `index_method`
    /// experimental feature, so the builder is always opened with it on — a
    /// database that carries an FTS index cannot be queried without it.
    pub async fn new(path: &str) -> Result<Self> {
        let db = Builder::new_local(path)
            .experimental_index_method(true)
            .build()
            .await
            .map_err(backend)?;
        let conn = db.connect().map_err(backend)?;
        conn.execute_batch(SCHEMA).await.map_err(backend)?;
        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// An unconstrained scope component (`None`) is stored as the empty string so
/// every row is one primary-key cell; the predicates in `search` then compare
/// plain values and never need to reason about NULL.
fn norm(component: &Option<String>) -> String {
    component.clone().unwrap_or_default()
}

/// Quote every whitespace-separated token of `query` and join them with an
/// explicit `AND`: the FTS parser sees only phrases (an AND-of-tokens) and
/// cannot mistake user text for `AND` / `OR` / `NOT` / `-` / `:` / `"`.
///
/// An embedded quote is backslash-escaped, which is the quote escape this
/// engine's phrase syntax accepts. Doubling the quote (the SQL idiom) instead
/// emits two adjacent empty phrases and a `missing space` parse error, so it
/// would turn a malformed query into an error — exactly what sanitizing must
/// not do.
fn sanitize_query(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\\\"")))
        .collect()
}

fn text_value(value: TursoValue) -> Result<String> {
    match value {
        TursoValue::Text(text) => Ok(text),
        _ => Err(StoreError::Backend("expected a TEXT column value".into())),
    }
}

fn real_value(value: TursoValue) -> Result<f64> {
    match value {
        TursoValue::Real(real) => Ok(real),
        TursoValue::Integer(int) => Ok(int as f64),
        _ => Err(StoreError::Backend(
            "expected a numeric fts_score value".into(),
        )),
    }
}

#[async_trait]
impl LexicalStore for TursoLexical {
    /// Index (or replace) the text of `id` within `scope`.
    async fn index(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO mem_lexical (ws, sess, task, fact_id, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                scope.workspace_id.clone(),
                norm(&scope.session_id),
                norm(&scope.task_id),
                id.0.clone(),
                text.to_string(),
            ),
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    /// Drop the (scope, id) row. A scope that never held `id` is not an
    /// error — recall arms drop ids speculatively.
    async fn drop(&self, scope: &ScopeKey, id: &FactId) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM mem_lexical
             WHERE ws = ?1 AND sess = ?2 AND task = ?3 AND fact_id = ?4",
            (
                scope.workspace_id.clone(),
                norm(&scope.session_id),
                norm(&scope.task_id),
                id.0.clone(),
            ),
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    /// Top `top_k` `(FactId, score)` hits for `query` within `scope`, best
    /// first. One statement does the FTS scan, the scope pre-filter, the
    /// ranking and the limit.
    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        let tokens = sanitize_query(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = tokens.join(" AND ");

        // Parameters are numbered because `?1` (the FTS pattern) is used by
        // both the filter and the ranking; the scope predicates are appended
        // only when the caller constrained that component.
        let mut sql = String::from(
            "SELECT fact_id, fts_score(text, ?1) FROM mem_lexical
             WHERE fts_match(text, ?1) AND ws = ?2",
        );
        let mut args = vec![
            TursoValue::Text(pattern),
            TursoValue::Text(scope.workspace_id.clone()),
        ];
        let mut next = 3;
        if let Some(session) = &scope.session_id {
            sql.push_str(&format!(" AND sess = ?{next}"));
            args.push(TursoValue::Text(session.clone()));
            next += 1;
        }
        if let Some(task) = &scope.task_id {
            sql.push_str(&format!(" AND task = ?{next}"));
            args.push(TursoValue::Text(task.clone()));
            next += 1;
        }
        sql.push_str(&format!(
            " ORDER BY fts_score(text, ?1) DESC, fact_id ASC LIMIT ?{next}"
        ));
        args.push(TursoValue::Integer(top_k as i64));

        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(&sql, params_from_iter(args))
            .await
            .map_err(backend)?;
        let mut out = Vec::with_capacity(top_k);
        while let Some(row) = rows.next().await.map_err(backend)? {
            let id = text_value(row.get_value(0).map_err(backend)?)?;
            let score = real_value(row.get_value(1).map_err(backend)?)?;
            out.push((FactId(id), score));
        }
        Ok(out)
    }
}
