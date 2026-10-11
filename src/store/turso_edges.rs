//! Turso adjacency table for fact-graph edges (stage doc 03 T4).
//!
//! One table `fact_edges(ws, sess, task, src, dst, rel, weight, source_event_id)`
//! primary-keyed per (scope, src, dst, rel) — so `upsert_edges` is an
//! `INSERT OR REPLACE` and the same (scope, src, dst, rel) is one logical row.
//! `scope` is part of the key, so one database file can hold many scopes.
//!
//! `neighbors` and `edges_by_rel` apply [`ScopeKey::admits`] *inside their own
//! query* (stage doc 03 contract 1): `ws` is always an equality predicate, and
//! `sess` / `task` are added only when the caller constrained them — an absent
//! constraint admits everything under the workspace. A narrow caller therefore
//! never sees another scope's rows.
//!
//! Traversal is **undirected**: each hop expands both outgoing (`src IN
//! frontier`) and incoming (`dst IN frontier`) adjacency in a single statement,
//! so an edge is reachable in either direction. A hop only expands the fact ids
//! it *newly* reached, which is what keeps a cyclic graph from re-walking old
//! territory — and the per-hop `IN` lists stay one frontier wide.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use turso::{params_from_iter, Builder, Connection, Row, Value as TursoValue};

use crate::core::traits::{FactEdgeStore, Result, StoreError};
use crate::core::types::{Edge, FactId, RelKind, ScopeKey};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS fact_edges (
     ws TEXT NOT NULL,
     sess TEXT NOT NULL,
     task TEXT NOT NULL,
     src TEXT NOT NULL,
     dst TEXT NOT NULL,
     rel TEXT NOT NULL,
     weight REAL NOT NULL,
     source_event_id TEXT NOT NULL,
     PRIMARY KEY (ws, sess, task, src, dst, rel)
 );
 CREATE INDEX IF NOT EXISTS fact_edges_src
     ON fact_edges (ws, sess, task, src)";

pub struct TursoFactEdges {
    _db: Arc<turso::Database>,
    conn: Arc<Mutex<Connection>>,
}

impl TursoFactEdges {
    /// Open (creating if needed) the database file at `path` and ensure the
    /// edge table and its src index exist. Both statements are idempotent.
    ///
    /// No FTS index is involved here, so the local engine is opened with its
    /// defaults — no experimental feature flags required.
    pub async fn new(path: &str) -> Result<Self> {
        let db = Builder::new_local(path).build().await.map_err(backend)?;
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
/// every row is one primary-key cell; the scope predicates in the read paths
/// then compare plain values and never need to reason about NULL.
fn norm(component: &Option<String>) -> String {
    component.clone().unwrap_or_default()
}

/// Stable spelling of [`RelKind`]: one string per variant, in declaration
/// order. The spelling *is* the value persisted in `rel`, so `rel_from_string`
/// is its exact inverse and the pair keeps a round-trip lossless (test 5).
fn rel_to_string(rel: RelKind) -> &'static str {
    match rel {
        RelKind::Supersedes => "Supersedes",
        RelKind::DependsOn => "DependsOn",
        RelKind::AboutTask => "AboutTask",
        RelKind::MentionsFile => "MentionsFile",
        RelKind::CausedBy => "CausedBy",
        RelKind::Corrects => "Corrects",
        RelKind::Related => "Related",
    }
}

/// Inverse of [`rel_to_string`]. A spelling this store never wrote is an
/// invariant violation, not a value to guess — surface it, never degrade.
fn rel_from_string(rel: &str) -> Result<RelKind> {
    Ok(match rel {
        "Supersedes" => RelKind::Supersedes,
        "DependsOn" => RelKind::DependsOn,
        "AboutTask" => RelKind::AboutTask,
        "MentionsFile" => RelKind::MentionsFile,
        "CausedBy" => RelKind::CausedBy,
        "Corrects" => RelKind::Corrects,
        "Related" => RelKind::Related,
        other => {
            return Err(StoreError::Backend(format!(
                "unknown rel spelling persisted in fact_edges: {other:?}"
            )))
        }
    })
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
            "expected a numeric weight value".into(),
        )),
    }
}

/// Decode one `fact_edges` row (src, dst, rel, weight, source_event_id).
fn edge_from_row(row: &Row) -> Result<Edge> {
    let src = text_value(row.get_value(0).map_err(backend)?)?;
    let dst = text_value(row.get_value(1).map_err(backend)?)?;
    let rel = rel_from_string(&text_value(row.get_value(2).map_err(backend)?)?)?;
    let weight = real_value(row.get_value(3).map_err(backend)?)?;
    let source_event_id = text_value(row.get_value(4).map_err(backend)?)?;
    Ok(Edge {
        src: FactId(src),
        dst: FactId(dst),
        rel,
        weight,
        source_event_id,
    })
}

/// Build one hop's undirected adjacency query: `src IN frontier OR dst IN
/// frontier`, under the caller's full scope predicate. `params` is appended in
/// exactly the numbered order the statement uses (`?1` = ws, …), so the
/// caller's placeholder numbering stays in lock-step with the pushed values.
fn adjacency_query(frontier: &[String], scope: &ScopeKey, params: &mut Vec<TursoValue>) -> String {
    params.push(TursoValue::Text(scope.workspace_id.clone()));
    let mut next = 2usize;
    let mut sql = String::from(
        "SELECT src, dst, rel, weight, source_event_id FROM fact_edges
         WHERE ws = ?1",
    );
    if let Some(session) = &scope.session_id {
        sql.push_str(&format!(" AND sess = ?{next}"));
        params.push(TursoValue::Text(session.clone()));
        next += 1;
    }
    if let Some(task) = &scope.task_id {
        sql.push_str(&format!(" AND task = ?{next}"));
        params.push(TursoValue::Text(task.clone()));
        next += 1;
    }
    // src list then dst list, each `frontier.len()` numbered placeholders.
    let start = next;
    let src_ph: Vec<String> = (0..frontier.len())
        .map(|i| format!("?{}", start + i))
        .collect();
    let dst_start = start + frontier.len();
    let dst_ph: Vec<String> = (0..frontier.len())
        .map(|i| format!("?{}", dst_start + i))
        .collect();
    for id in frontier.iter().chain(frontier.iter()) {
        params.push(TursoValue::Text(id.clone()));
    }
    sql.push_str(&format!(
        " AND (src IN ({src_ph}) OR dst IN ({dst_ph}))
          ORDER BY src, dst, rel",
        src_ph = src_ph.join(", "),
        dst_ph = dst_ph.join(", "),
    ));
    sql
}

/// Append the caller's optional `sess` / `task` predicates to `sql`, numbering
/// them from `start` and pushing matching `params`. Shared by the read paths
/// that filter on a single `src` or `rel` equality rather than an adjacency set.
fn scope_predicates(
    sql: &mut String,
    start: usize,
    scope: &ScopeKey,
    params: &mut Vec<TursoValue>,
) {
    let mut next = start;
    if let Some(session) = &scope.session_id {
        sql.push_str(&format!(" AND sess = ?{next}"));
        params.push(TursoValue::Text(session.clone()));
        next += 1;
    }
    if let Some(task) = &scope.task_id {
        sql.push_str(&format!(" AND task = ?{next}"));
        params.push(TursoValue::Text(task.clone()));
    }
}

#[async_trait]
impl FactEdgeStore for TursoFactEdges {
    /// Insert or replace each edge under `scope`; the row's scope cells are the
    /// writer's (`None` → `""`), not the reader's. An empty batch is a no-op.
    async fn upsert_edges(&self, scope: &ScopeKey, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        for edge in edges {
            conn.execute(
                "INSERT OR REPLACE INTO fact_edges
                 (ws, sess, task, src, dst, rel, weight, source_event_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    scope.workspace_id.clone(),
                    norm(&scope.session_id),
                    norm(&scope.task_id),
                    edge.src.0.clone(),
                    edge.dst.0.clone(),
                    rel_to_string(edge.rel),
                    edge.weight,
                    edge.source_event_id.clone(),
                ),
            )
            .await
            .map_err(backend)?;
        }
        Ok(())
    }

    /// Drop every edge written by `scope` out of `src`. A scope that never had
    /// such an edge is not an error — the graph arm removes ids speculatively.
    /// Only outgoing edges are removed; incoming edges stay.
    async fn remove_edges_from(&self, scope: &ScopeKey, src: &FactId) -> Result<()> {
        let conn = self.conn.lock().await;
        let mut sql = String::from(
            "DELETE FROM fact_edges
             WHERE ws = ?1 AND src = ?2",
        );
        let mut params: Vec<TursoValue> = vec![
            TursoValue::Text(scope.workspace_id.clone()),
            TursoValue::Text(src.0.clone()),
        ];
        scope_predicates(&mut sql, 3, scope, &mut params);
        conn.execute(&sql, params_from_iter(params))
            .await
            .map_err(backend)?;
        Ok(())
    }

    /// Bounded n-hop undirected neighborhood of `seeds` under `scope`. One
    /// statement per hop expands the whole frontier in both directions; edges
    /// are deduplicated by (src, dst, rel) and a hop only walks ids it newly
    /// reached, so a cycle terminates. `hops = 0` or empty `seeds` → empty.
    async fn neighbors(&self, seeds: &[FactId], hops: u8, scope: &ScopeKey) -> Result<Vec<Edge>> {
        if seeds.is_empty() || hops == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().await;

        // Fact ids already reached: the seeds (hop 0) plus every id first seen
        // at a later hop. `insert` returning `false` is the loop guard.
        let mut reached: BTreeSet<String> = seeds.iter().map(|seed| seed.0.clone()).collect();
        let mut frontier: Vec<String> = seeds.iter().map(|seed| seed.0.clone()).collect();

        // Keys of edges already returned, so an edge reachable on two hops is
        // reported exactly once.
        let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();

        let mut out: Vec<Edge> = Vec::new();

        for _hop in 0..hops {
            if frontier.is_empty() {
                break;
            }

            let mut params: Vec<TursoValue> = Vec::new();
            let sql = adjacency_query(&frontier, scope, &mut params);

            let mut rows = conn
                .query(&sql, params_from_iter(params))
                .await
                .map_err(backend)?;
            let mut next_frontier: Vec<String> = Vec::new();

            while let Some(row) = rows.next().await.map_err(backend)? {
                let edge = edge_from_row(&row)?;
                let (src_id, dst_id) = (edge.src.0.clone(), edge.dst.0.clone());
                let key = (
                    src_id.clone(),
                    dst_id.clone(),
                    rel_to_string(edge.rel).to_string(),
                );
                if !seen_edges.insert(key) {
                    continue;
                }
                // Expand both endpoints; only ids never reached before advance
                // the frontier, which is what stops a cycle from re-walking.
                for id in [src_id, dst_id] {
                    if reached.insert(id.clone()) {
                        next_frontier.push(id);
                    }
                }
                out.push(edge);
            }

            frontier = next_frontier;
        }

        Ok(out)
    }

    /// Every edge under `scope` carrying `rel`, ordered by (src, dst, rel).
    async fn edges_by_rel(&self, scope: &ScopeKey, rel: RelKind) -> Result<Vec<Edge>> {
        let conn = self.conn.lock().await;
        let mut sql = String::from(
            "SELECT src, dst, rel, weight, source_event_id FROM fact_edges
             WHERE ws = ?1 AND rel = ?2",
        );
        let mut params: Vec<TursoValue> = vec![
            TursoValue::Text(scope.workspace_id.clone()),
            TursoValue::Text(rel_to_string(rel).to_string()),
        ];
        scope_predicates(&mut sql, 3, scope, &mut params);
        sql.push_str(" ORDER BY src, dst, rel");

        let mut rows = conn
            .query(&sql, params_from_iter(params))
            .await
            .map_err(backend)?;
        let mut out: Vec<Edge> = Vec::new();
        while let Some(row) = rows.next().await.map_err(backend)? {
            out.push(edge_from_row(&row)?);
        }
        Ok(out)
    }
}
