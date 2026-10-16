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
//!   contract 1), drops non-current rows (`Fact::is_current`: superseded or
//!   `valid_to`-closed), and scores by the same
//!   deterministic lexical overlap the memory backend uses, so the two
//!   backends are interchangeable in the recall tests;
//! - `supersede` is the bi-temporal correction path: the old row survives
//!   with `superseded_by` set, `valid_to` closed at the correction's
//!   application time and `revision` bumped, and leaves `search`.
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

/// `session_id` / `task_id` are optional scope components, normalised to the
/// empty string so no PK column is ever NULL: SQLite does not enforce
/// uniqueness on a composite key containing NULL (`NULL != NULL`), so a
/// scope with any absent component would silently accumulate duplicate rows
/// under repeated `INSERT OR REPLACE` and break the idempotent-upsert
/// recovery protocol. The empty string is unrepresentable as a real
/// component (ids are non-empty), so it is a safe sentinel.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS facts (
    workspace_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    fact_id TEXT NOT NULL,
    body TEXT NOT NULL,
    superseded_by TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, session_id, task_id, fact_id)
)";

fn backend(error: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(error.to_string())
}

/// An absent optional scope component, normalised to the empty string so no
/// PK column is ever NULL (see [`SCHEMA`]).
fn norm(component: &Option<String>) -> String {
    component.clone().unwrap_or_default()
}

fn text(value: TursoValue) -> String {
    match value {
        TursoValue::Text(text) => text,
        other => format!("{other:?}"),
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
    /// Every component is compared with plain `= ?N` because an absent
    /// optional is normalised to the empty string on write. An earlier
    /// version emitted `IS NULL` for the absent case and did not normalise
    /// the write, which made the PK (and therefore `INSERT OR REPLACE`)
    /// ineffective for those scopes; see [`SCHEMA`].
    fn scope_clause(scope: &ScopeKey, start: usize) -> (String, Vec<TursoValue>) {
        let mut conditions = vec![format!("workspace_id = ?{start}")];
        let mut params = vec![TursoValue::Text(scope.workspace_id.clone())];
        for (offset, (column, value)) in [
            ("session_id", norm(&scope.session_id)),
            ("task_id", norm(&scope.task_id)),
        ]
        .into_iter()
        .enumerate()
        {
            let index = start + 1 + offset;
            conditions.push(format!("{column} = ?{index}"));
            params.push(TursoValue::Text(value));
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
                norm(&fact.scope.session_id),
                norm(&fact.scope.task_id),
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
        //
        // The scan is scoped so its result set is dropped before anything
        // else runs on the connection: the driver holds an exclusive
        // operation for an open row stream, and a live one makes the next
        // statement fail with "connection is busy with another operation".
        let located = {
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
            text(column(&row, 0)?)
        };
        // Readers decode the row's `body` JSON, not the typed columns, so a
        // correction must rewrite the body too — otherwise `get` hands back
        // the pre-correction record even though the columns updated.
        let mut previous: Fact = serde_json::from_str(&located)
            .map_err(|error| backend(format!("decoding fact: {error}")))?;
        previous.superseded_by = Some(new.id.clone());
        previous.valid_to = Some(new.created_at);
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
        // Both statements land together or neither does: a crash between the
        // UPDATE and the INSERT would leave the old row pointing at a fact id
        // that does not exist — superseded out of search, with the new fact
        // absent and the Supersedes edge dangling at it. The repo already
        // wraps multi-statement writes this way (`TursoKv::upsert`).
        conn.execute_batch("BEGIN").await.map_err(backend)?;
        let updated = conn
            .execute(
                &format!(
                    "UPDATE facts SET superseded_by = ?1, revision = ?2, body = ?3 \
                     WHERE {clause} AND fact_id = ?{id_index}"
                ),
                update_params,
            )
            .await;
        let inserted = match updated {
            Ok(_) => {
                conn.execute(
                    "INSERT OR REPLACE INTO facts
                        (workspace_id, session_id, task_id, fact_id, body, superseded_by, revision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        new.scope.workspace_id.clone(),
                        norm(&new.scope.session_id),
                        norm(&new.scope.task_id),
                        new.id.0.clone(),
                        serde_json::to_string(new).map_err(backend)?,
                        new.superseded_by.clone().map(|id| id.0),
                        new.revision
                    ],
                )
                .await
            }
            Err(error) => Err(error),
        };
        match inserted {
            Ok(_) => conn.execute_batch("COMMIT").await.map_err(backend)?,
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK").await;
                return Err(backend(error));
            }
        }
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
            // `ScopeKey::admits`: an absent query constraint admits every
            // row under the workspace, so it contributes no condition; a
            // present one is compared for equality against the normalised
            // stored value.
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
            // `valid_to` lives inside the body, so the temporal half of
            // `Fact::is_current` is enforced here; the superseded half is
            // already filtered in SQL.
            if !fact.is_current() {
                continue;
            }
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
