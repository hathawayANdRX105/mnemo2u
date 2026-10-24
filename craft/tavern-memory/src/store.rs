//! The tavern craft's store wiring: write the canon and the session's events.
//!
//! The three stores land under a directory the caller names. The same layout
//! the other crafts use, because they all write the same engine:
//!
//! | store | file | role |
//! |---|---|---|
//! | turso | `facts.db` / `lexical.db` / `edges.db` | truth: facts, lexical index, relation edges |
//! | lancedb | `vectors/` | derived: fact vectors (rebuildable from truth) |
//!
//! The embedder is local ONNX (fastembed), default `BAAI/bge-m3` — the one
//! model the whole repo shares, so one download serves every craft.
//!
//! ## What a canon fact is for
//!
//! A canon entry is *project knowledge*: "she never smokes" holds for the
//! character regardless of which session observed it. So it drops the session
//! id, and a character-wide recall (`session_id = None`) reads back the whole
//! canon — which is exactly what a world-book rendering wants. An event is the
//! opposite: it happened inside one session, so it keeps the session id.
//!
//! [`ScopeKey::admits`] is strict in one direction — a query that *names* a
//! session only sees rows stored with that same session, while a query that
//! leaves it empty sees everything under the workspace. That is what makes the
//! split work in both directions.
//!
//! ## What a conflict is not
//!
//! A conflict is reported, never auto-resolved. The craft finds that an event
//! and a canon entry collide; which one survives is a judgement about the
//! fiction, so it belongs to the judge. The write path therefore never
//! supersedes a canon fact because an event disagreed with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use mnemo2u::core::traits::{Embedder, FactEdgeStore, FactStore, FactVectorStore, LexicalStore};
use mnemo2u::core::types::{Fact, FactId, ScopeKey, SourceRef, TrustState};
use mnemo2u::llm::fastembed::FastembedEmbedder;
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_facts::TursoFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;

use crate::config::Config;
use crate::{Conflict, TavernMemory, TavernScope};

/// The content hash of a canon entry's text.
///
/// The entry's own hash: the narrative kernel's slot scheme, so the same
/// rescan logic holds here as in the novel craft.
#[must_use]
pub fn content_hash(text: &str) -> String {
    narrative_core::hash::content_hash(text)
}

/// Join a slot key and its content hash into a `SourceRef.event_id`.
#[must_use]
pub fn event_id(slot_key: &str, text: &str) -> String {
    format!("{slot_key}#{}", content_hash(text))
}

/// Recover the content hash from a `SourceRef.event_id`.
#[must_use]
pub fn hash_of_event_id(event_id: &str) -> Option<&str> {
    narrative_core::hash::hash_of_event_id(event_id)
}

/// The store directory's conventional location under a character root.
#[must_use]
pub fn default_store_root(character_dir: &Path) -> PathBuf {
    character_dir.join("research").join("memo")
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

/// What a write pass should produce: the facts, ready to commit.
#[derive(Debug, Clone, Default)]
pub struct WritePlan {
    /// The facts to write, in write order.
    pub facts: Vec<Fact>,
}

/// Plan one session's writes: full scopes, full ids, no I/O.
///
/// Pure, so the scope arithmetic is testable without the embedder. The I/O
/// path is [`Coder::record`].
#[must_use]
pub fn plan(memory: &TavernMemory, scope: &TavernScope, created_at_base: i64) -> WritePlan {
    let mut written = WritePlan::default();

    // Canon: project knowledge, so no session id.
    // Canon: the same session scope the lib's own `canon_fact` uses. A canon
    // fact does *not* drop its session id here -- `ScopeKey::admits` only lets a
    // query see a row when a present constraint matches exactly, and a stored
    // `None` is invisible to a query that names a session. Dropping it would
    // make the canon unreachable from the very session that wrote it. A
    // character-wide recall is what `TavernScope::to_character_scope` is for,
    // and that is the caller's choice to make, not this writer's.
    let canon_scope = scope.to_scope_key();
    for entry in memory.canon.values() {
        written.facts.push(fact(
            FactId(entry.key.clone()),
            canon_scope.clone(),
            crate::FactKind::Canon.as_str(),
            &entry.text,
            // The card's own file is the provenance, which is where a human
            // goes to read the sheet the statement came from.
            Some(entry.source_path.clone()),
            &entry.key,
            &entry.text,
            None,
            created_at_base,
        ));
    }

    // Events: session-bound, so the session id is the whole point.
    let event_scope = ScopeKey {
        workspace_id: scope.character.clone(),
        session_id: Some(scope.session.clone()),
        task_id: None,
    };
    for event in memory.events.values() {
        written.facts.push(fact(
            FactId(event.key.clone()),
            event_scope.clone(),
            crate::FactKind::Event.as_str(),
            &event.text,
            Some(event.source_path.clone()),
            &event.key,
            &event.text,
            // Story time is the event's position in the fiction's own
            // timeline, which is not the order the transcript was written in.
            Some(event.tick),
            created_at_base,
        ));
    }

    written
}

/// Build one fact from an entry's parts.
#[allow(clippy::too_many_arguments)]
fn fact(
    id: FactId,
    scope: ScopeKey,
    kind: &str,
    text: &str,
    provenance: Option<String>,
    slot_key: &str,
    body: &str,
    valid_from: Option<narrative_core::storytime::Tick>,
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
            branch_id: provenance.unwrap_or_default(),
            epoch: 0,
            event_id: event_id(slot_key, body),
        }],
        trust_state: TrustState::Verified,
        valid_from,
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

    /// Plan and write one session's memory.
    ///
    /// A canon fact is never superseded *because* an event disagreed with it.
    /// The text changing is what triggers a supersede; a disagreement is a
    /// conflict to report, which is [`crate::detect`]'s job.
    pub async fn record(&self, memory: &TavernMemory, scope: &TavernScope) -> Result<WriteReport> {
        let planned = plan(memory, scope, self.config.created_at_base);
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

/// The conflicts one session's events raise against the canon.
///
/// A thin re-export of the detector, so a caller that has just written a
/// session can ask what it contradicted without importing the conflict module.
#[must_use]
pub fn conflicts_of(memory: &TavernMemory) -> Vec<Conflict> {
    crate::detect(memory.canon.values(), memory.events.values())
}
