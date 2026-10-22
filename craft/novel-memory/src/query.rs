//! 记忆层的读路径：语义检索、关系查询、按 story 时间线的事实流。
//!
//! 三个查询的时序语义不同：
//!
//! - [`query`]：语义检索，跨全部 story（`task_id = None`）
//! - [`relations`]：某实体的关系边 + 关系演进事实
//! - [`timeline`]：按 story 序号排出的事实流——**不是**按写作顺序
//!
//! 三者都只读当前版本的事实：mnemo2u 的 `FactStore::search` 已经把被
//! supersede 掉的旧版本排除在检索结果之外（`valid_to` 闭合的行不进
//! search），所以这里不需要再过滤一遍。

use anyhow::{anyhow, Result};
use mnemo2u::core::types::{Edge, Fact, FactId, ScopeKey};

use crate::corpus::story_number;
use crate::store::MemoryBackend;
use crate::storytime::Layer;

/// 一条命中。
#[derive(Debug, Clone)]
pub struct Hit {
    /// 事实 id（槽位键）。
    pub id: String,
    /// 事实正文。
    pub text: String,
    /// 相似度（越大越近）。
    pub score: f64,
    /// 所属层。
    pub layer: Layer,
    /// story 序号（`None` = 全书级）。
    pub story: Option<i64>,
    /// 证据文件与行号。
    pub source: String,
}

impl Hit {
    /// 一行摘要。
    pub fn line(&self) -> String {
        let story = match self.story {
            Some(number) => format!("story {number}"),
            None => "全书".to_string(),
        };
        format!(
            "- [{}] {} `{}` {:.3}\n  {}",
            self.layer.label(),
            story,
            self.id,
            self.score,
            self.source
        )
    }
}

/// 语义检索。
///
/// 走向量臂（`FactVectorStore::search`）——它按 scope 硬前滤，跨全部 story
/// 是把 `task_id` 放空。
pub async fn query(
    backend: &MemoryBackend,
    scope: &ScopeKey,
    question: &str,
    limit: usize,
) -> Result<Vec<Hit>> {
    let embedding = backend
        .embedder
        .embed(&[question.to_string()])
        .await
        .map_err(|error| anyhow!("嵌入失败: {error}"))?;
    let vector = embedding
        .first()
        .ok_or_else(|| anyhow!("嵌入器返回空向量"))?;

    let hits = backend
        .vectors
        .search(scope, vector, limit)
        .await
        .map_err(|error| anyhow!("向量检索失败: {error}"))?;

    let mut out = Vec::with_capacity(hits.len());
    for (id, score) in hits {
        let Some(fact) = backend
            .facts
            .get(scope, &id)
            .await
            .map_err(|error| anyhow!("读取事实失败: {error}"))?
        else {
            continue;
        };
        out.push(hit_of(&fact, score));
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// 某实体的关系。
///
/// 边是跨 story 成立的，所以查边用全书作用域；演进事实（关系状态的变化）
/// 挂在各自的 story 下，靠 `valid_from` 排序。
pub async fn relations(
    backend: &MemoryBackend,
    scope: &ScopeKey,
    entity: &str,
) -> Result<Vec<Hit>> {
    let book = ScopeKey {
        workspace_id: scope.workspace_id.clone(),
        session_id: scope.session_id.clone(),
        task_id: None,
    };
    let seed = FactId(entity.to_string());
    let edges = backend
        .edges
        .neighbors(&[seed], 1, &book)
        .await
        .map_err(|error| anyhow!("查关系边失败: {error}"))?;

    let mut out = Vec::new();
    for edge in edges {
        let other = if edge.src.0 == entity {
            &edge.dst
        } else {
            &edge.src
        };
        out.push(Hit {
            id: format!("{} -> {}", edge.src.0, edge.dst.0),
            text: format!("{} → {}（权重 {:.2}）", edge.src.0, edge.dst.0, edge.weight),
            score: edge.weight,
            layer: Layer::Relation,
            story: None,
            source: edge.source_event_id.clone(),
        });
        let _ = other;
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// 按 story 时间线的事实流。
///
/// 排序键是 `valid_from`（story 序号），不是写入顺序也不是章节号——
/// 插叙章的章节号比它叙述的事件晚，但它的事实落在更早的 story 时刻。
pub async fn timeline(
    backend: &MemoryBackend,
    scope: &ScopeKey,
    unit: Option<&str>,
) -> Result<Vec<Hit>> {
    let facts = backend
        .facts
        .scan(scope)
        .await
        .map_err(|error| anyhow!("扫描事实失败: {error}"))?;

    let mut out: Vec<Hit> = facts
        .iter()
        .filter(|fact| fact.is_current())
        .map(|fact| {
            let mut hit = hit_of(fact, 1.0);
            hit.story = fact.valid_from.map(story_number);
            hit
        })
        .filter(|hit| match unit {
            Some(unit) => hit.story.map(story_number) == unit.parse::<i64>().ok(),
            None => true,
        })
        .collect();

    // 排序：story 序号 → 事实 id（同一 story 内按槽位键的字典序，位置稳定）。
    out.sort_by(|a, b| a.story.cmp(&b.story).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

/// 事实 → 命中。
fn hit_of(fact: &Fact, score: f64) -> Hit {
    let layer = Layer::of_fact_id(&fact.id.0);
    let source = fact
        .source_refs
        .first()
        .map(|reference| reference.event_id.clone())
        .unwrap_or_default();
    Hit {
        id: fact.id.0.clone(),
        text: fact.text.clone(),
        score,
        layer,
        story: fact.valid_from.map(story_number),
        source,
    }
}

/// 边 → 文本（报告用）。
pub fn edge_line(edge: &Edge) -> String {
    format!("{} → {}", edge.src.0, edge.dst.0)
}
