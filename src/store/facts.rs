//! In-process fact store for the memory-layer truth surface (stage doc 03).
//!
//! `MemoryFactStore` is the deterministic backend the recall contracts are
//! written against: if scope pre-filtering, supersede and the audit ledger
//! hold here, they are testable without a durable store.
//!
//! Row semantics:
//!
//! - rows are keyed by `(ScopeKey, FactId)`, so `put` overwrites in place
//!   and one `FactId` may exist once per scope;
//! - `get` matches the scope **exactly**: a foreign or a merely narrower
//!   request gets `None`, never another scope's row and not even a hint that
//!   it exists;
//! - `search` applies [`ScopeKey::admits`] *inside* the store (stage doc 03
//!   contract 1), drops superseded rows, and scores by deterministic lexical
//!   overlap of the query against the fact's `text`;
//! - `supersede` is the bi-temporal correction path: the old row survives
//!   with `superseded_by` set and `revision` bumped, and leaves `search`.
//!
//! Derivative: nothing here is durable; rebuild from the truth layer.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use async_trait::async_trait;

use crate::core::traits::{FactStore, Result, StoreError};
use crate::core::types::{Fact, FactId, ScopeKey};

/// Rows keyed by `(scope, id)` plus the append-only audit ledger. One lock
/// covers both so an audit entry can never describe a row state that is not
/// in the map.
#[derive(Default)]
struct Inner {
    rows: HashMap<(ScopeKey, FactId), Fact>,
    audit: Vec<(String, String)>,
}

/// In-memory [`FactStore`].
#[derive(Default)]
pub struct MemoryFactStore {
    inner: RwLock<Inner>,
}

impl MemoryFactStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the audit ledger as `(op, payload_hash)` pairs in append
    /// order.
    pub fn audit(&self) -> Vec<(String, String)> {
        self.inner.read().expect("fact store lock").audit.clone()
    }
}

/// Lowercase tokens split on non-alphanumerics — the same tokenisation the
/// mock embedder uses (`llm/mock.rs`), so lexical overlap and the mock's
/// embedding scores stay comparable.
fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Deterministic lexical score: distinct shared tokens over distinct query
/// tokens. A query with no tokens, or a fact sharing none, scores 0.
fn score(query: &str, text: &str) -> f64 {
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return 0.0;
    }
    let fact_tokens = tokenize(text);
    let shared = query_tokens
        .iter()
        .filter(|token| fact_tokens.contains(*token))
        .count();
    shared as f64 / query_tokens.len() as f64
}

#[async_trait]
impl FactStore for MemoryFactStore {
    async fn put(&self, fact: &Fact) -> Result<()> {
        let mut inner = self.inner.write().expect("fact store lock");
        inner
            .rows
            .insert((fact.scope.clone(), fact.id.clone()), fact.clone());
        Ok(())
    }

    async fn get(&self, scope: &ScopeKey, id: &FactId) -> Result<Option<Fact>> {
        let inner = self.inner.read().expect("fact store lock");
        Ok(inner.rows.get(&(scope.clone(), id.clone())).cloned())
    }

    async fn supersede(&self, old: &FactId, new: &Fact) -> Result<()> {
        let mut inner = self.inner.write().expect("fact store lock");
        let key = inner
            .rows
            .iter()
            .find(|(_, row)| &row.id == old)
            .map(|(key, _)| key.clone())
            .ok_or_else(|| StoreError::NotFound(old.0.clone()))?;
        let row = inner.rows.get_mut(&key).expect("row located above");
        row.superseded_by = Some(new.id.clone());
        row.revision += 1;
        inner
            .rows
            .insert((new.scope.clone(), new.id.clone()), new.clone());
        Ok(())
    }

    async fn search(
        &self,
        scope: &ScopeKey,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(FactId, f64)>> {
        let inner = self.inner.read().expect("fact store lock");
        let mut hits: Vec<(FactId, f64)> = inner
            .rows
            .values()
            .filter(|row| scope.admits(&row.scope) && row.superseded_by.is_none())
            .map(|row| (row.id.clone(), score(query, &row.text)))
            .collect();
        // Highest score first; ties break on the id so the same corpus yields
        // the same sequence on every call.
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0 .0.cmp(&b.0 .0)));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn append_audit(&self, op: &str, payload_hash: &str) -> Result<()> {
        let mut inner = self.inner.write().expect("fact store lock");
        inner.audit.push((op.to_string(), payload_hash.to_string()));
        Ok(())
    }
}
