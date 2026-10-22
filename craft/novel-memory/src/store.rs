//! mnemo2u 后端装配：把三库开在小说目录里，把语料写进去，再读出来。
//!
//! 三库落在 `novel1/research/memo/`：
//!
//! | 库 | 文件 | 角色 |
//! |---|---|---|
//! | turso | `facts.db` / `lexical.db` / `edges.db` | 真值层：事实、词法索引、关系边 |
//! | lancedb | `vectors/` | 派生层：事实向量（可从真值层重建） |
//!
//! 嵌入器是本地 ONNX（fastembed），模型名可配，缺省 `BAAI/bge-m3`（1024 维）。
//! **不发外网**：模型只在第一次使用时下载到本地缓存，之后离线推理。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use mnemo2u::core::traits::{Embedder, FactEdgeStore, FactStore, FactVectorStore, LexicalStore};
use mnemo2u::core::types::{Edge, Fact, FactId, RelKind, ScopeKey, SourceRef, TrustState};
use mnemo2u::llm::fastembed::FastembedEmbedder;
use mnemo2u::store::lance_facts::LanceFactVector;
use mnemo2u::store::turso_edges::TursoFactEdges;
use mnemo2u::store::turso_facts::TursoFactStore;
use mnemo2u::store::turso_lexical::TursoLexical;

use crate::config::Config;
use crate::corpus::Corpus;
use crate::storytime::Layer;

/// 记忆后端：三库 + 嵌入器。
pub struct MemoryBackend {
    /// 真值层：事实。
    pub facts: Arc<dyn FactStore>,
    /// 真值层：词法索引。
    pub lexical: Arc<dyn LexicalStore>,
    /// 真值层：关系边。
    pub edges: Arc<dyn FactEdgeStore>,
    /// 派生层：事实向量。
    pub vectors: Arc<dyn FactVectorStore>,
    /// 本地嵌入器。
    pub embedder: Arc<dyn Embedder>,
    /// 三库根目录。
    pub root: PathBuf,
}

impl MemoryBackend {
    /// 打开（或建）小说目录下的三库。
    pub async fn open(root: &Path, memory: &Config) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("创建记忆目录失败: {}", root.display()))?;

        let facts = TursoFactStore::open(&root.join("facts.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("打开事实库失败: {error}"))?;
        let lexical = TursoLexical::new(&root.join("lexical.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("打开词法索引失败: {error}"))?;
        let edges = TursoFactEdges::new(&root.join("edges.db").to_string_lossy())
            .await
            .map_err(|error| anyhow!("打开关系库失败: {error}"))?;

        let embedder: Arc<dyn Embedder> = Arc::new(
            FastembedEmbedder::new(&memory.embedding_model)
                .map_err(|error| anyhow!("加载嵌入模型失败: {error}"))?,
        );
        let vectors = LanceFactVector::new(&root.join("vectors").to_string_lossy(), embedder.dim())
            .await
            .map_err(|error| anyhow!("打开向量库失败: {error}"))?;

        Ok(Self {
            facts: Arc::new(facts),
            lexical: Arc::new(lexical),
            edges: Arc::new(edges),
            vectors: Arc::new(vectors),
            embedder,
            root: root.to_path_buf(),
        })
    }

    /// 嵌入器的维度。
    pub fn dim(&self) -> usize {
        self.embedder.dim()
    }
}

/// 写入报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    /// 写入的事实数。
    pub written: usize,
    /// 幂等命中（内容没变，只刷新了向量）。
    pub unchanged: usize,
    /// 修正数（旧事实被新事实取代）。
    pub superseded: usize,
    /// 写入的关系边数。
    pub edges: usize,
    /// 三层各自的事实数。
    pub by_layer: Vec<(&'static str, usize)>,
}

/// 语料写入后端。
///
/// 幂等靠**槽位键 + 内容哈希**：同一个槽位键在库里已有、且正文与哈希
/// 一致，就不重复写；正文变了，就把旧事实 supersede 掉、写新的。
pub struct Indexer<'a> {
    backend: &'a MemoryBackend,
    scope: ScopeKey,
    memory: Config,
}

impl<'a> Indexer<'a> {
    /// 造一个写入器。
    pub fn new(backend: &'a MemoryBackend, scope: ScopeKey, memory: Config) -> Self {
        Self {
            backend,
            scope,
            memory,
        }
    }

    /// 把语料写进三库。
    pub async fn index(&self, corpus: &Corpus) -> Result<IndexReport> {
        let mut report = IndexReport::default();
        let mut counts = [0usize; 3];

        // The new facts are collected and written in one batch: the store
        // commits per row, so a novel's worth of facts would otherwise be a
        // queue of thousands of transactions.
        let mut fresh: Vec<Fact> = Vec::new();
        for slot in &corpus.slots {
            // id 是**槽位键**（位置），不带内容哈希：id 一变，`supersede` 就
            // 定位不到旧行，修正链会断。正文变没变记在 `SourceRef.event_id`。
            let fact_id = FactId(slot.key.clone());
            let scope = ScopeKey {
                workspace_id: self.scope.workspace_id.clone(),
                session_id: self.scope.session_id.clone(),
                task_id: slot.task_id.clone().or_else(|| self.scope.task_id.clone()),
            };
            let existing = self
                .backend
                .facts
                .get(&scope, &fact_id)
                .await
                .map_err(|error| anyhow!("读取事实失败: {error}"))?;

            let current_hash = crate::hash::content_hash(&slot.text);
            let unchanged = existing.as_ref().is_some_and(|fact| {
                fact.source_refs.iter().any(|reference| {
                    crate::hash::hash_of_event_id(&reference.event_id)
                        .is_some_and(|hash| hash == current_hash)
                })
            });

            match existing {
                Some(_) if unchanged => {
                    report.unchanged += 1;
                    self.refresh_vectors(&scope, &fact_id, &slot.text).await?;
                }
                Some(fact) => {
                    // A correction replaces the row it supersedes, so it goes
                    // through `supersede` rather than the batch.
                    let mut replacement = self.build_fact(&fact_id, &scope, slot);
                    replacement.revision = fact.revision + 1;
                    self.backend
                        .facts
                        .supersede(&fact_id, &replacement)
                        .await
                        .map_err(|error| anyhow!("取代事实失败: {error}"))?;
                    self.index_lexical(&scope, &fact_id, &slot.text).await?;
                    self.write_vector(&scope, &fact_id, &slot.text).await?;
                    report.superseded += 1;
                    report.written += 1;
                }
                None => {
                    fresh.push(self.build_fact(&fact_id, &scope, slot));
                }
            }
            counts[layer_index(slot.layer)] += 1;
        }

        if !fresh.is_empty() {
            self.backend
                .facts
                .put_many(&fresh)
                .await
                .map_err(|error| anyhow!("批量写入事实失败: {error}"))?;
            report.written += fresh.len();
            for fact in &fresh {
                let scope = fact.scope.clone();
                self.index_lexical(&scope, &fact.id, &fact.text).await?;
                self.write_vector(&scope, &fact.id, &fact.text).await?;
            }
        }

        report.by_layer = vec![
            ("设定", counts[0]),
            ("事实", counts[1]),
            ("关系", counts[2]),
        ];
        report.edges = self.write_edges(corpus).await?;
        Ok(report)
    }

    /// 造一条事实。
    fn build_fact(
        &self,
        fact_id: &FactId,
        scope: &ScopeKey,
        slot: &crate::corpus::FactSlot,
    ) -> Fact {
        Fact {
            id: fact_id.clone(),
            scope: scope.clone(),
            kind: slot.layer.label().to_string(),
            text: slot.text.clone(),
            source_refs: vec![SourceRef {
                workspace_id: scope.workspace_id.clone(),
                session_id: scope.session_id.clone().unwrap_or_default(),
                branch_id: "main".to_string(),
                epoch: 0,
                event_id: crate::hash::event_id(&slot.key, &slot.text),
            }],
            trust_state: TrustState::Verified,
            valid_from: slot.valid_from,
            valid_to: None,
            created_at: self.memory.created_at_base,
            superseded_by: None,
            revision: 0,
        }
    }

    /// 写词法索引。
    async fn index_lexical(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        self.backend
            .lexical
            .index(scope, id, text)
            .await
            .map_err(|error| anyhow!("写词法索引失败: {error}"))
    }

    /// 写向量。
    async fn write_vector(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        let embedding = self
            .backend
            .embedder
            .embed(&[text.to_string()])
            .await
            .map_err(|error| anyhow!("嵌入失败: {error}"))?;
        let vector = embedding
            .first()
            .ok_or_else(|| anyhow!("嵌入器返回空向量"))?;
        self.backend
            .vectors
            .upsert(scope, id, vector)
            .await
            .map_err(|error| anyhow!("写向量失败: {error}"))
    }

    /// 内容没变时只刷新向量（幂等重扫的快路径）。
    async fn refresh_vectors(&self, scope: &ScopeKey, id: &FactId, text: &str) -> Result<()> {
        self.write_vector(scope, id, text).await
    }

    /// 写关系边。
    async fn write_edges(&self, corpus: &Corpus) -> Result<usize> {
        let mut edges = Vec::new();
        for edge in &corpus.edges {
            let src = self.resolve_endpoint(&edge.src, corpus);
            let dst = self.resolve_endpoint(&edge.dst, corpus);
            let (Some(src), Some(dst)) = (src, dst) else {
                continue;
            };
            edges.push(Edge {
                src: FactId(src),
                dst: FactId(dst),
                rel: edge.rel,
                weight: edge.weight,
                source_event_id: edge.source_path.to_string_lossy().into_owned(),
            });
        }
        let count = edges.len();
        if count == 0 {
            return Ok(0);
        }
        // 边挂在全书作用域下：关系是跨 story 成立的，挂在某个 story 下会
        // 让「这两人有什么关系」只在那个 story 里可见。
        let scope = ScopeKey {
            workspace_id: self.scope.workspace_id.clone(),
            session_id: self.scope.session_id.clone(),
            task_id: None,
        };
        self.backend
            .edges
            .upsert_edges(&scope, &edges)
            .await
            .map_err(|error| anyhow!("写关系边失败: {error}"))?;
        Ok(count)
    }

    /// 边端点：槽位键 → 事实 id。
    ///
    /// 卡片端点直接是卡级事实 id；`entity:` 端点指向一张卡（按名字找），
    /// 找不到就指向一个以名字为键的「关系事实」——关系的另一端必须有一个
    /// 事实承载，否则图上的边指向空。
    fn resolve_endpoint(&self, endpoint: &str, corpus: &Corpus) -> Option<String> {
        if let Some(card) = corpus.cards.iter().find(|card| card.key == endpoint) {
            return Some(self.fact_id_of_card(card));
        }
        if let Some(name) = endpoint.strip_prefix("entity:") {
            if let Some(card) = corpus.find_card(name) {
                return Some(self.fact_id_of_card(card));
            }
            // 关系端点也用稳定 id：槽位键 + 名字，不带内容哈希。
            return Some(format!("entity:{name}"));
        }
        None
    }

    /// 卡级事实 id（与写入时一致：稳定槽位键，不带内容哈希）。
    fn fact_id_of_card(&self, card: &crate::corpus::CardInfo) -> String {
        card.key.clone()
    }
}

/// 层 → 计数下标。
fn layer_index(layer: Layer) -> usize {
    match layer {
        Layer::Setting => 0,
        Layer::Fact => 1,
        Layer::Relation => 2,
    }
}

/// 关系类型的中文名（报告用）。
pub fn rel_label(rel: RelKind) -> &'static str {
    match rel {
        RelKind::Supersedes => "取代",
        RelKind::DependsOn => "依赖",
        RelKind::AboutTask => "关于",
        RelKind::MentionsFile => "提及",
        RelKind::CausedBy => "导致",
        RelKind::Corrects => "修正",
        RelKind::Related => "相关",
    }
}
