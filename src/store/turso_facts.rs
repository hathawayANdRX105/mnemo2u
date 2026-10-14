//! Turso-backed [`FactStore`] — the memory layer's durable truth surface.
//!
//! `MemoryFactStore` (store/facts.rs) defines the row semantics this
//! backend must reproduce exactly, because the recall contracts are written
//! against them:
//!
//! - rows are keyed by `(ScopeKey, FactId)`, so `put` overwrites in place
//!   and one `FactId` exists once per scope;
//! - `get` matches the scope **exactly** — a foreign or merely narrower
//!   request gets `None`, never another scope's row;
//! - `search` applies [`ScopeKey::admits`] *inside* the store (stage doc 03
//!   contract 1), drops superseded rows, and scores by the same
//!   deterministic lexical overlap the memory backend uses, so the two
//!   backends are interchangeable in the recall tests;
//! - `supersede` is the bi-temporal correction path: the old row survives
//!   with `superseded_by` set and `revision` bumped, and leaves `search`.
//!
//! The scope predicate is one shared SQL `WHERE` clause rather than a Rust
//! post-filter: the stage doc's contract is that the filter applies inside
//! the store's own query, never over unscoped ranks (a narrower caller must
//! not even see another scope's rank positions).
//!
//! Schema: `facts(workspace_id, session_id, task_id, fact_id, body,
//! superseded_by, revision)` with the primary key carrying the scope
//! columns first, matching the logical key order.

use std::sync::Arc;

use async_trait::async_trait;

use tokio::sync::Mutex;
use turso::{params, Builder, Connection, Value as TursoValue};

use crate::core::traits::{FactStore, Result, StoreError};
use crate::core::types::{Fact, FactId, ScopeKey};

/// `session_id` / `task_id` are optional scope components; SQL `NULL` is the
/// absent case, so the column is nullable and the exact-match rule below
/// still holds (`NULL` never equals a bound value).
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS facts (
    workspace_id TEXT NOT NULL,
    session_id TEXT,
    task_id TEXT,
    fact_id TEXT NOT NULL,
    body TEXT NOT NULL,
    superseded_by TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, session_id, task_id, fact_id)
)";

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

fn text(value: TursoValue) -> String {
    match value {
        TursoValue::Text(text) => text,
        other => format!("{other:?}"),
    }
}

fn integer(value: TursoValue) -> i64 {
    match value {
        TursoValue::Integer(number) => number,
        _ => 0,
    }
}

/// `get_value` returns the driver's own error; map it like the others.
fn column(row: &turso::Row, index: usize) -> Result<turso::Value> {
    row.get_value(index).map_err(backend)
}

/// The durable fact store.
pub struct TursoFactStore {
    _db: Arc<turso::Database>,
    conn: Arc<Mutex<Connection>>,
}

impl TursoFactStore {
    /// Open (creating if needed) a database file and the facts table.
    pub async fn open(path: &str) -> Result<Self> {
        let db = Builder::new_local(path).build().await.map_err(backend)?;
        let conn = db.connect().map_err(backend)?;
        conn.execute_batch(SCHEMA).await.map_err(backend)?;
        Ok(Self {
            _db: Arc::new(db),
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Build the scope predicate and its bound parameters, with parameter
    /// numbers starting at `start` so callers can place their own values
    /// before the clause.
    ///
    /// An absent optional scope component is written as the literal
    /// `IS NULL` rather than compared against a bound parameter: the driver's
    /// null-aware operator with a parameter did not match a NULL column, so
    /// the clause is generated from the values instead of relying on the
    /// engine to compare them. A present component is `= ?N`.
    fn scope_clause(scope: &ScopeKey, start: usize) -> (String, Vec<TursoValue>) {
        let mut conditions = vec![format!("workspace_id = ?{start}")];
        let mut params = vec![TursoValue::Text(scope.workspace_id.clone())];
        let mut index = start + 1;
        for (column, value) in [
            ("session_id", &scope.session_id),
            ("task_id", &scope.task_id),
        ] {
            match value {
                Some(bound) => {
                    conditions.push(format!("{column} = ?{index}"));
                    params.push(TursoValue::Text(bound.clone()));
                    index += 1;
                }
                None => conditions.push(format!("{column} IS NULL")),
            }
        }
        (conditions.join(" AND "), params)
    }
}

#[async_trait]
impl FactStore for TursoFactStore {
    async fn put(&self, fact: &Fact) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO facts
                (workspace_id, session_id, task_id, fact_id, body, superseded_by, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                fact.scope.workspace_id.clone(),
                fact.scope.session_id.clone(),
                fact.scope.task_id.clone(),
                fact.id.0.clone(),
                serde_json::to_string(fact).map_err(backend)?,
                fact.superseded_by.clone().map(|id| id.0),
                fact.revision
            ],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn get(&self, scope: &ScopeKey, id: &FactId) -> Result<Option<Fact>> {
        let conn = self.conn.lock().await;
        let (clause, mut scope_params) = Self::scope_clause(scope, 1);
        let id_index = scope_params.len() + 1;
        scope_params.push(TursoValue::Text(id.0.clone()));
        let mut rows = conn
            .query(
                &format!("SELECT body FROM facts WHERE {clause} AND fact_id = ?{id_index}"),
                scope_params,
            )
            .await
            .map_err(backend)?;
        let row = rows.next().await.map_err(backend)?;
        match row {
            Some(row) => {
                let body = text(column(&row, 0)?);
                serde_json::from_str(&body)
                    .map(Some)
                    .map_err(|error| backend(format!("decoding fact: {error}")))
            }
            None => Ok(None),
        }
    }

    async fn supersede(&self, old: &FactId, new: &Fact) -> Result<()> {
        let conn = self.conn.lock().await;
        // Locate the row by its own scope: the caller supplies the fact id,
        // and the correction must land on the row that actually holds it
        // (the memory backend searches every scope for the id too).
        let mut located = conn
            .query(
                "SELECT body FROM facts WHERE fact_id = ?1",
                params![old.0.clone()],
            )
            .await
            .map_err(backend)?;
        let row = located.next().await.map_err(backend)?;
        let Some(row) = row else {
            return Err(StoreError::NotFound(old.0.clone()));
        };
        // Readers decode the row's `body` JSON, not the typed columns, so a
        // correction must rewrite the body too — otherwise `get` hands back
        // the pre-correction record even though the columns updated.
        let mut previous: Fact = serde_json::from_str(&text(column(&row, 0)?))
            .map_err(|error| backend(format!("decoding fact: {error}")))?;
        previous.superseded_by = Some(new.id.clone());
        previous.revision += 1;
        let body = serde_json::to_string(&previous).map_err(backend)?;

        // ?1 superseded_by, ?2 revision, ?3 body, then the row's own scope
        // (workspace + present optionals), then the fact id.
        let (clause, scope_params) = Self::scope_clause(&previous.scope, 4);
        let id_index = scope_params.len() + 4;
        let mut update_params = vec![
            TursoValue::Text(new.id.0.clone()),
            TursoValue::Integer(previous.revision as i64),
            TursoValue::Text(body),
        ];
        update_params.extend(scope_params);
        update_params.push(TursoValue::Text(old.0.clone()));
        conn.execute(
            &format!(
                "UPDATE facts SET superseded_by = ?1, revision = ?2, body = ?3 \
                 WHERE {clause} AND fact_id = ?{id_index}"
            ),
            update_params,
        )
        .await
        .map_err(backend)?;

        conn.execute(
            "INSERT OR REPLACE INTO facts
                (workspace_id, session_id, task_id, fact_id, body, superseded_by, revision)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                new.scope.workspace_id.clone(),
                new.scope.session_id.clone(),
                new.scope.task_id.clone(),
                new.id.0.clone(),
                serde_json::to_string(new).map_err(backend)?,
                new.superseded_by.clone().map(|id| id.0),
                new.revision
            ],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }

    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        let conn = self.conn.lock().await;
        // The admitting predicate, mirroring `ScopeKey::admits` exactly:
        // the workspace matches, and a **bound** optional component must
        // equal the row's, while an **absent** one imposes nothing at all
        // (it admits every row under the workspace). That asymmetry is why
        // the clause is generated per component rather than shared.
        let mut conditions = vec!["workspace_id = ?1".to_string()];
        let mut binds = vec![TursoValue::Text(scope.workspace_id.clone())];
        let mut index = 2usize;
        for (column, value) in [
            ("session_id", &scope.session_id),
            ("task_id", &scope.task_id),
        ] {
            if let Some(bound) = value {
                conditions.push(format!("{column} = ?{index}"));
                binds.push(TursoValue::Text(bound.clone()));
                index += 1;
            }
        }
        conditions.push("superseded_by IS NULL".to_string());
        let sql = format!("SELECT body FROM facts WHERE {}", conditions.join(" AND "));
        let mut rows = conn.query(&sql, binds).await.map_err(backend)?;
        let mut hits: Vec<(FactId, f64)> = Vec::new();
        while let Some(row) = rows.next().await.map_err(backend)? {
            let body = text(column(&row, 0)?);
            let fact: Fact = serde_json::from_str(&body)
                .map_err(|error| backend(format!("decoding fact: {error}")))?;
            hits.push((
                fact.id.clone(),
                crate::store::facts::score(query, &fact.text),
            ));
        }
        // Same ordering contract as the memory backend: score descending,
        // id ascending for ties.
        hits.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0 .0.cmp(&right.0 .0))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    async fn append_audit(&self, op: &str, payload_hash: &str) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS fact_audit (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                op TEXT NOT NULL,
                payload_hash TEXT NOT NULL
            )",
            (),
        )
        .await
        .map_err(backend)?;
        conn.execute(
            "INSERT INTO fact_audit (op, payload_hash) VALUES (?1, ?2)",
            params![op.to_string(), payload_hash.to_string()],
        )
        .await
        .map_err(backend)?;
        Ok(())
    }
}

/// The store's own document count, for `admin status`-style reporting.
pub async fn count(store: &TursoFactStore) -> Result<usize> {
    let conn = store.conn.lock().await;
    let mut rows = conn
        .query("SELECT COUNT(*) FROM facts", ())
        .await
        .map_err(backend)?;
    let row = rows.next().await.map_err(backend)?;
    Ok(row
        .map(|row| row.get_value(0).map(integer).unwrap_or(0) as usize)
        .unwrap_or(0))
}
