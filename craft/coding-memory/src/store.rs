//! The coding craft's store wiring: write what a session remembered.
//!
//! The three stores land under a directory the caller names (a repository's
//! `research/memo/`, or wherever the harness keeps its state):
//!
//! | store | file | role |
//! |---|---|---|
//! | turso | `facts.db` / `lexical.db` / `edges.db` | truth: facts, lexical index, relation edges |
//! | lancedb | `vectors/` | derived: fact vectors (rebuildable from truth) |
//!
//! The embedder is local ONNX (fastembed), model configurable, default
//! `BAAI/bge-m3` — the same model the novel craft uses, so one download serves
//! both. Nothing leaves the machine after the first run.
//!
//! ## What is planned and what is written
//!
//! [`plan`] is pure: given a session record and a task scope it says which
//! facts and which edges *should* exist, with fully resolved scopes and ids.
//! [`Coder::record`] performs the writes. The split exists because opening the
//! vector store needs the embedder, which needs the model — so the scope
//! arithmetic, the id derivation and the idempotency decision are all testable
//! without an ONNX download, while the I/O itself stays thin enough to read as
//! one glance.
//!
//! ## Idempotency
//!
//! A fact's id is its **slot key** — `turn:7`, `rule:lockfile` — never its
//! text, because an id that moves orphans the row `supersede` would look for.
//! Whether the text changed is recorded as a hash in `SourceRef.event_id`, the
//! same scheme the novel craft uses. Rescanning the same session therefore
//! refreshes vectors instead of growing a chain.
//!
//! ## Scopes, and why rules drop the session
//!
//! `ScopeKey::admits` is strict in one direction: a query that *names* a
//! session or a task only sees rows stored with that same value, while a query
//! that leaves it empty sees everything under the workspace. So:
//!
//! - an **episode** is session-bound: it keeps the session id, because a pit
//!   hit in one run is not a fact about the project;
//! - a **rule** or a **procedure** is project knowledge: it drops the session
//!   id, or a project-wide recall could never read back what any one session
//!   learned. Both keep the topic, so a topic-scoped query also finds them.
//! - an **edge** records derivation — which experiment a rule came from — which
//!   is project knowledge, so it drops both the session and the topic.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use mnemo2u::core::text::compute_mdhash_id;
use mnemo2u::core::traits::{Embedder, FactEdgeStore, FactStore, FactVectorStore, LexicalStore};
use mnemo2u::core::types::{Edge, Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::llm::fastembed::FastembedEmbedder;
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_facts::TursoFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;

use crate::config::Config;
use crate::{Episode, Procedure, Rule, TaskScope};

/// The content hash of a fact's text.
///
/// The engine's own stable hashing, rather than a hand-rolled one: the digest
/// is compared against a previously written row inside this store, and a
/// hasher that can change across Rust versions would make an unchanged
/// correction look like one.
#[must_use]
pub fn content_hash(text: &str) -> String {
    compute_mdhash_id(text, "")
}

/// Join a slot key and its content hash into a `SourceRef.event_id`.
#[must_use]
pub fn event_id(slot_key: &str, text: &str) -> String {
    format!("{slot_key}#{}", content_hash(text))
}

/// Recover the content hash from a `SourceRef.event_id`.
///
/// `None` when the id carries no hash: an id written by something else, which
/// the caller must treat as "unknown" rather than "unchanged".
#[must_use]
pub fn hash_of_event_id(event_id: &str) -> Option<&str> {
    event_id.rsplit_once('#').map(|(_, hash)| hash)
}

/// The store directory's conventional location under a project root.
///
/// A caller may name any directory; this is the one used when nobody says
/// otherwise, mirroring the novel craft's `research/memo` convention.
#[must_use]
pub fn default_store_root(project_dir: &Path) -> PathBuf {
    project_dir.join("research").join("memo")
}

/// The three stores plus the embedder.
pub struct MemoryBackend {
    /// Truth: facts.
    pub facts: Arc<dyn FactStore>,
    /// Truth: lexical index.
    pub lexical: Arc<dyn LexicalStore>,
    /// Truth: relation edges.
    pub edges: Arc<dyn FactEdgeStore>,
    /// Derived: fact vectors.
    pub vectors: Arc<dyn FactVectorStore>,
    /// The local embedder.
    pub embedder: Arc<dyn Embedder>,
    /// The stores' root directory.
    pub root: PathBuf,
}

impl MemoryBackend {
    /// Open (or create) the three stores under `root`.
    pub async fn open(root: &Path, config: &Config) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("create the memory directory: {}", root.display()))?;

        let facts = TursoFactStore::open(&root.join("facts.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("open the fact store: {error}"))?;
        let lexical = TursoLexical::new(&root.join("lexical.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("open the lexical index: {error}"))?;
        let edges = TursoFactEdges::new(&root.join("edges.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("open the edge store: {error}"))?;

        let embedder: Arc<dyn Embedder> = Arc::new(
            FastembedEmbedder::new(&config.embedding_model)
                .map_err(|error| anyhow!("load the embedding model: {error}"))?,
        );
        let vectors = LanceFactVector::new(&root.join("vectors").to_string_lossy(), embedder.dim())
            .await
            .map_err(|error| anyhow!("open the vector store: {error}"))?;

        Ok(Self {
            facts: Arc::new(facts),
            lexical: Arc::new(lexical),
            edges: Arc::new(edges),
            vectors: Arc::new(vectors),
            embedder,
            root: root.to_path_buf(),
        })
    }

    /// The embedder's dimension.
    pub fn dim(&self) -> usize {
        self.embedder.dim()
    }
}

/// Everything one session remembered: what happened, what holds, what to repeat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRecord {
    /// The episodes, in the order they happened.
    pub episodes: Vec<Episode>,
    /// The rules, each naming the episode it came from when it has one.
    pub rules: Vec<Rule>,
    /// The procedures, each naming the rule it came from when it has one.
    pub procedures: Vec<Procedure>,
}

/// What a write pass should produce: the facts and the edges, ready to commit.
///
/// No `PartialEq` derive: that would need one on `Fact`, and the engine's
/// fact type deliberately does not have it. Comparisons in tests read the
/// fields they care about instead.
#[derive(Debug, Clone, Default)]
pub struct WritePlan {
    /// The facts to write, in write order.
    pub facts: Vec<Fact>,
    /// The edges to write, pointing at those facts.
    pub edges: Vec<Edge>,
}

/// Plan one session's writes: full scopes, full ids, no I/O.
///
/// This is the function to read to learn what the craft actually writes. The
/// scopes are the interesting part — see the module docs for why a rule drops
/// its session id and an edge drops everything but the workspace.
#[must_use]
pub fn plan(session: &SessionRecord, scope: &TaskScope, created_at_base: i64) -> WritePlan {
    let mut written = WritePlan::default();

    for episode in &session.episodes {
        // Session-bound: an episode is what one run did.
        let fact_scope = ScopeKey {
            workspace_id: scope.project.clone(),
            session_id: Some(scope.session.clone()),
            task_id: scope.topic.clone(),
        };
        written.facts.push(fact(
            FactId(episode.fact_id()),
            fact_scope,
            crate::MemoryKind::Episode.as_str(),
            &episode.text,
            episode.turn_id.clone(),
            &episode.key,
            created_at_base,
        ));
    }

    for rule in &session.rules {
        // The session id stays. `ScopeKey::admits` is strict in the direction
        // that bites: a query that *names* a session sees only rows stored with
        // that same session, so a stored `None` is invisible to the session
        // that wrote it — the tavern craft learned that the hard way. Keeping it
        // costs nothing: a project-wide recall names no session, and `None` on
        // the query side admits everything. So this way both readers work.
        let fact_scope = scope.to_scope_key();
        written.facts.push(fact(
            FactId(rule.key.clone()),
            fact_scope,
            crate::MemoryKind::Rule.as_str(),
            &rule.text,
            rule.derived_from.clone(),
            &rule.key,
            created_at_base,
        ));
    }

    for procedure in &session.procedures {
        // Same as a rule: the session stays. See the note above.
        let fact_scope = scope.to_scope_key();
        written.facts.push(fact(
            FactId(procedure.key.clone()),
            fact_scope,
            crate::MemoryKind::Procedure.as_str(),
            &procedure.body,
            procedure.derived_from.clone(),
            &procedure.key,
            created_at_base,
        ));
    }

    // Edge endpoints are fact **ids**, not slot keys. A rule's `derived_from`
    // names the episode's slot key, and an episode written from a turn is
    // filed under `turn:<turn_id>` — so the endpoint has to be resolved, or
    // the edge points at a row that does not exist.
    let episode_fact_ids: std::collections::HashMap<&str, String> = session
        .episodes
        .iter()
        .map(|episode| (episode.key.as_str(), episode.fact_id()))
        .collect();
    let rule_fact_ids: std::collections::HashMap<&str, &str> = session
        .rules
        .iter()
        .map(|rule| (rule.key.as_str(), rule.key.as_str()))
        .collect();

    for rule in &session.rules {
        if let Some(episode_key) = &rule.derived_from {
            // An unresolvable endpoint means the episode is not in this
            // record: skip the edge rather than write a dangling one.
            if let Some(episode_fact_id) = episode_fact_ids.get(episode_key.as_str()) {
                written.edges.push(Edge {
                    src: FactId(rule.key.clone()),
                    dst: FactId(episode_fact_id.clone()),
                    rel: crate::rule_depends_on_kind(),
                    weight: 1.0,
                    source_event_id: rule.key.clone(),
                });
            }
        }
    }
    for procedure in &session.procedures {
        if let Some(rule_key) = &procedure.derived_from {
            if rule_fact_ids.contains_key(rule_key.as_str()) {
                written.edges.push(Edge {
                    src: FactId(procedure.key.clone()),
                    dst: FactId(rule_key.clone()),
                    rel: crate::procedure_from_rule_kind(),
                    weight: 1.0,
                    source_event_id: procedure.key.clone(),
                });
            }
        }
    }

    written
}

/// Build one fact from an item's parts.
fn fact(
    id: FactId,
    scope: ScopeKey,
    kind: &str,
    text: &str,
    provenance: Option<String>,
    slot_key: &str,
    created_at_base: i64,
) -> Fact {
    Fact {
        id,
        scope: scope.clone(),
        kind: kind.to_string(),
        text: text.to_string(),
        source_refs: vec![SourceRef {
            workspace_id: scope.workspace_id,
            session_id: scope.session_id.unwrap_or_default(),
            // The provenance rides here, so the reasoning stays traceable
            // without a column the engine does not have.
            branch_id: provenance.unwrap_or_default(),
            epoch: 0,
            event_id: event_id(slot_key, text),
        }],
        trust_state: TrustState::Verified,
        valid_from: None,
        valid_to: None,
        created_at: created_at_base,
        superseded_by: None,
        revision: 0,
    }
}

/// What one write pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteReport {
    /// Facts written for the first time.
    pub written: usize,
    /// Facts whose text had not changed (vectors refreshed only).
    pub unchanged: usize,
    /// Facts that replaced an older row through `supersede`.
    pub superseded: usize,
    /// Edges written.
    pub edges: usize,
}

/// The writer: turns a plan into committed rows, idempotently.
pub struct Coder<'a> {
    backend: &'a MemoryBackend,
    config: Config,
}

impl<'a> Coder<'a> {
    /// Build a writer over an open backend.
    pub fn new(backend: &'a MemoryBackend, config: Config) -> Self {
        Self { backend, config }
    }

    /// Plan and write one session record.
    pub async fn record(&self, session: &SessionRecord, scope: &TaskScope) -> Result<WriteReport> {
        let planned = plan(session, scope, self.config.created_at_base);
        let mut report = WriteReport::default();
        let mut fresh: Vec<Fact> = Vec::new();

        for fact in &planned.facts {
            let existing = self
                .backend
                .facts
                .get(&fact.scope, &fact.id)
                .await
                .map_err(|error| anyhow!("read the fact store: {error}"))?;
            let unchanged = existing.as_ref().is_some_and(|old| {
                old.source_refs.iter().any(|reference| {
                    hash_of_event_id(&reference.event_id)
                        .is_some_and(|hash| hash == content_hash(&fact.text))
                })
            });

            match existing {
                Some(_) if unchanged => {
                    report.unchanged += 1;
                    self.refresh_vectors(&fact.scope, &fact.id, &fact.text)
                        .await?;
                }
                Some(old) => {
                    let mut replacement = fact.clone();
                    replacement.revision = old.revision + 1;
                    replacement.created_at = self.config.created_at_base + old.revision as i64 + 1;
                    self.backend
                        .facts
                        .supersede(&fact.id, &replacement)
                        .await
                        .map_err(|error| anyhow!("supersede the fact: {error}"))?;
                    self.index_lexical(&fact.scope, &fact.id, &fact.text)
                        .await?;
                    self.write_vector(&fact.scope, &fact.id, &fact.text).await?;
                    report.superseded += 1;
                }
                None => fresh.push(fact.clone()),
            }
        }

        if !fresh.is_empty() {
            self.backend
                .facts
                .put_many(&fresh)
                .await
                .map_err(|error| anyhow!("write the facts: {error}"))?;
            report.written += fresh.len();
            for fact in &fresh {
                self.index_lexical(&fact.scope, &fact.id, &fact.text)
                    .await?;
                self.write_vector(&fact.scope, &fact.id, &fact.text).await?;
            }
        }

        if !planned.edges.is_empty() {
            let edge_scope = ScopeKey {
                workspace_id: scope.project.clone(),
                session_id: None,
                task_id: None,
            };
            self.backend
                .edges
                .upsert_edges(&edge_scope, &planned.edges)
                .await
                .map_err(|error| anyhow!("write the edges: {error}"))?;
            report.edges = planned.edges.len();
        }

        Ok(report)
    }

    /// Write the lexical index row.
    async fn index_lexical(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        self.backend
            .lexical
            .index(scope, id, text)
            .await
            .map_err(|error| anyhow!("write the lexical index: {error}"))
    }

    /// Embed and write the vector.
    async fn write_vector(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        let embedding = self
            .backend
            .embedder
            .embed(&[text.to_string()])
            .await
            .map_err(|error| anyhow!("embed the text: {error}"))?;
        let vector = embedding
            .first()
            .ok_or_else(|| anyhow!("the embedder returned an empty vector"))?;
        self.backend
            .vectors
            .upsert(scope, id, vector)
            .await
            .map_err(|error| anyhow!("write the vector: {error}"))
    }

    /// The fast path for an unchanged fact: refresh the derived vector only.
    async fn refresh_vectors(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        self.write_vector(scope, id, text).await
    }
}
