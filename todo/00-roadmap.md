# mnemo2u 路线图（施工图 v1）

**状态**：v1 待评审；`crates/mnemo-core` 类型与 trait 骨架已建、`cargo check` 通过；R1 未开工。
**目标**：为 coding harness 提供独立的记忆/RAG 库——graph RAG 存关联、向量 RAG 存事实、Turso 存真值，Jev 承担语义判断；公开仓、独立开发。
**基线**：本文件 + 分阶段文档（01–06）；参考项目在 `todo/refs/`。

## 文档索引

| 文档 | 阶段 | 内容 |
|---|---|---|
| [01-nano-graphrag-port](01-nano-graphrag-port.md) | R1 | **复刻 nano-graphrag**：切块→抽取→合并→建图→社区→三模式查询 + 三库提交协议 |
| [02-lightrag-upgrade](02-lightrag-upgrade.md) | R2 | **LightRAG 增强**：增量更新/双层检索/关键词标签/合并升级/选择性删除/成本控制 |
| [03-retrieval](03-retrieval.md) | R3 | 读路径硬化：scope 前滤→多臂→RRF→rerank→分层返回 |
| [04-jev-integration](04-jev-integration.md) | R4 | Jev 全链接入与外发门 |
| [05-reflect](05-reflect.md) | R5 | 反思更新：corroborate/supersede/失效传播 |
| [06-evaluation](06-evaluation.md) | R6 | 评测：MTRAG/LongMemEval/自建 case + manifest |
| [refs/](refs/) | — | 参考项目克隆与论文 PDF（见 §10） |

阅读顺序：新会话先读本文件 → 当前阶段文档 → 其前置阶段的接续记录。不依赖聊天口头结论。
**开发形态**：单 crate（`src/` 子目录划分大功能：`core` / `store` / `llm` / `graph` / `index` / `query`），需要时再拆 workspace。

## 1. 命名与定位

- 名字：`mnemo2u`（crates.io 与 GitHub 全站已核，均未占用）。
- 定位：harness 记忆层的独立实现，与 mono 仓 `todo/context-compact` 的 FSD-CC 机制对接：
  - C01 原文真值（SessionDb）→ mnemo2u 只存派生条目 + `SourceRef`，不存原文副本。
  - C02 预算 → 检索结果计入 `BudgetAllocation`。
  - C04 `task_id` → 图的锚点。
  - C05 四工具 → mnemo2u 是 `context_search` 的语义后端。
  - C06 外发门与 `organized_through` 水位 → 写路径复用同契约。
  - C08 manifest → R6 评测复用同格式。

## 2. 架构总览（三库）

```
写路径 (mnemo-index)
  closed turn 切片 → 本地候选(零-LLM) → [高价值才过 Jev]
  → 治理写网关（去重/冲突/tombstone）→ 按提交协议落三库
        │
        ├─→ turso   真值+审计+双时态+BM25(Tantivy FTS)   ← 唯一可重建源
        ├─→ kuzu    图/边/多跳/PageRank（冻结 0.11.3）    ← 派生，可重建
        └─→ lancedb 向量 ANN（0.40）                      ← 派生，可重建

读路径 (mnemo-query)
  scope 硬前滤 → 向量臂 + BM25 臂 + 图邻接臂 [+时间臂]
  → RRF 融合 → jev_rerank → exists_verdict
  → L0 摘要先行，L2 原文按需 read（SourceRef 回 harness）
```

**可重建性是核心不变量**：turso 是唯一真值；kuzu/lancedb 是派生索引，任何时候可以从 turso 全量重建。三库不共享事务，跨库一致性靠提交协议（见 01）。

## 3. 借鉴来源（逐条标注，含许可证）

| 借什么 | 来源 | 许可 | 本地副本（todo/refs/） | 落到 |
|---|---|---|---|---|
| 组件 trait 骨架（LLM/Embedding/Graph/Vector/KV 可插拔） | nano-graphrag | MIT | `nano-graphrag/` @ acb35c0 | `src/core`（已映射）；R1 全面复刻 |
| 双层检索、增量更新、关系关键词标签 | LightRAG (EMNLP'25) | MIT | `LightRAG/` @ 453dce8 | R2 |
| 骨架式索引：只对高价值 chunk 抽取（成本-1 个数量级） | KET-RAG (KDD'25) | 论文 | `papers/ket-rag.pdf` | R2 成本控制 |
| 实体/关系/claims 图模型、local/global 检索划分 | Microsoft GraphRAG | MIT | `graphrag/` @ 5faaaf4 | R1 对齐参考 |
| 双时态边（闭区间不删） | Zep/Graphiti | Apache-2.0 | `graphiti/`；`papers/zep.pdf` | `Fact`/`Edge` 字段（已建） |
| trust-state / 写网关 / tombstone / evidence-before-belief / RRF / scope 前置 | agent-memory-atlas | MIT | `agent-memory-atlas/`（克隆中） | R2/R3/R5 |
| 时间邻接检索（时间关联触发/衰减） | SynapticRAG | 论文 | `papers/synapticrag.pdf` | R3 可选时间臂 |
| 分层后台记忆树、多路径检索 | TRACE | 待核 | `TRACE/`（克隆中） | R5 形态参考 |
| retain/recall/reflect | Hindsight | MIT | `hindsight/`（稀疏）；`papers/hindsight.pdf` | R5 |
| 多轮评测基线（110 段） | MTRAG (IBM) | 待核 | `mt-rag-benchmark/`（克隆中）；`papers/mtrag.pdf` | R6 |
| 7 阶段管线全景、配置驱动 | graphrag-rs | MIT | `graphrag-rs/` @ a7222c1 | R1 参考 |
| PPR 图召回实现参考 | fast-graphrag | MIT | `fast-graphrag/`（克隆中） | R3 |
| 记忆层全景参考（Mem0 等实现对比） | Mem0 等 | 各自 | `mem0/`（克隆中）；`papers/mem0.pdf` | R5 参考 |

**红线**：OpenViking 为 AGPLv3——只借概念（L0/L1/L2 分层）不抄代码；所有借鉴保留出处与许可。

## 4. 技术栈

- **语言/形态**：Rust 2021，**单 crate**（`src/{core,store,llm,graph,index,query}`），需要时再拆。
- **CLI**：`clap` 4.x（`insert` / `query` / `admin` 子命令，调试与管理入口）。
- **图数据库**：**`kuzu` 0.11.3**（嵌入式 Cypher、列存、进程内；上游已归档 → 冻结版 + `GraphStore` trait 兜底）。
- **图算法**：`leiden-rs` 0.8（层次社区检测，R1 社区）、`petgraph` 0.8（内存图辅助/算法）；PPR 自实现（参考 `refs/fast-graphrag`，R3）。
- **向量库**：`lancedb` 0.40（嵌入式 ANN）。
- **真值库**：`turso` 0.8.x（beta；SQLite 兼容、原生 async、Tantivy FTS BM25）。
- **嵌入**：`fastembed` 7.x（ONNX 本地，bge-small 系；离线；模型名进缓存键）。
- **中文分词**：`jieba-rs` 0.11（切块 token 近似、FTS 辅助）。
- **HTTP/LLM**：`reqwest` + `serde_json`（OpenAI-compatible 生成客户端；Jev compatible 判断客户端）。
- **异步/基础设施**：`tokio`；错误 `thiserror`；哈希 `sha2`（幂等/缓存键）；日志 `tracing` + `tracing-subscriber`；配置 `serde` + `toml`。

## 5. 数据契约（骨架已落地）

- `Fact{ id, scope, kind(semantic|episodic|procedural), text, source_refs[], trust_state, valid_from, valid_to, created_at, superseded_by, revision }`
- `Edge{ src, dst, rel, weight, source_event_id }`（R2 增列 `timestamp, conversation_id`）
- `ScopeKey{ workspace_id, session_id?, task_id? }`：进主键、进每个索引、进 embedding cache key。
- `TrustState`：candidate / verified / rejected / stale；默认只放行 verified。
- 三条不变量：① 派生索引可重建；② 单臂失败必须与空结果可区分；③ tombstone 只在写路径检查、不进召回不进 prompt。

## 6. 阶段路线（摘要）

依赖链：**R1 → R2 → R3 → R4 → R5 → R6**，严格串行（R4 依赖 R2+R3 双侧接缝；R5 依赖 R4 的判断能力）。

| 阶段 | 可交付行为 | 验收要点 |
|---|---|---|
| R1 | **复刻 nano-graphrag**：insert/merge/图/社区/报告/local-global-naive 查询 + 三库提交协议 | 功能验收 FAC1–5；测试验收 TAC1–6 |
| R2 | **LightRAG 增强**：增量更新、双层检索、关键词、合并、选择性删除、成本控制 | 功能验收 FAC1–6；测试验收 TAC1–8 |
| R3 | 读路径硬化：前滤→多臂→融合→rerank→分层返回 | 精确召回、scope 负例、降级可观测 |
| R4 | Jev 全链 + 外发门 | 未 opt-in 零外发；无 Jev 显式降级 |
| R5 | 反思更新与后台整理 | 纠正抗重建、独立来源计数、级联失效 |
| R6 | 评测体系 | 冻结判据后跑分；三轴报告 |

**规模策略**：第一批只索引「最近 50 轮会话 + 显式 pin 文档」；闭环稳定后按水位增量扩全量，**不做全量重建**。

## 7. 已定决策记录

| 决策 | 结论 | 依据 |
|---|---|---|
| 存储形态 | 三库结合（turso+kuzu+lancedb） | 用户拍板（2026-10-09） |
| 图库 | kuzu 0.11.3 冻结 + `GraphStore` trait 兜底（**待用户点头**） | jev opt_d；上游归档风险已知 |
| 向量库 | lancedb（弃 qdrant-edge beta） | jev opt_d |
| 词法 | turso Tantivy FTS（非 FTS5 语法：`fts_match`/`fts_score`） | 官方 COMPAT.md 核实 |
| 写路径形态 | 零-LLM 捕获 + Jev 选择性增强（KET-RAG 式） | 调研 + 用户路线 |
| 命名 | mnemo2u | 用户拍板 |

## 8. 开放风险与待办

1. **kuzu 上游归档**：接受冻结版 + trait 抽象（替换成本收敛在 `src/store` 一个文件）。待点头。
2. **turso beta**（0.8.3-pre.1）：API 可能漂移；钉版本 + 契约测试护栏。
3. **参考库扩充**：论文 11/11 已落地；新增克隆（agent-memory-atlas / TRACE / mt-rag-benchmark / HippoRAG / fast-graphrag / mem0）后台下载中（见 §10）。TGRAG 论文未核实，其数字不采信。
4. **Jev 调用成本模型**：写路径每次抽取的调用上限与缓存策略在 R4 定稿。
5. **TRACE 与 MTRAG 许可**：使用前核实。

## 9. 接续记录

- 已完成：单 crate 骨架 + 三库/词法/命名决策 + 核心参考库（7/7）+ 路线图 + R1/R2 全文文档。
- 未完成：评审、扩充克隆与论文、R1 开工。
- 下一动作：评审 00–06 → 修订 → R1。

## 10. 参考库索引（todo/refs/）

**项目克隆**：

| 目录 | 仓库 | 状态 |
|---|---|---|
| `nano-graphrag/` | gusye1234/nano-graphrag | ✅ acb35c0 |
| `LightRAG/` | HKUDS/LightRAG | ✅ 453dce8 |
| `graphrag/` | microsoft/graphrag | ✅ 5faaaf4 |
| `graphrag-rs/` | automataIA/graphrag-rs | ✅ a7222c1 |
| `graphiti/` | getzep/graphiti | ✅ 1026ae7 |
| `hindsight/` | vectorize-io/hindsight（稀疏） | ✅ |
| `agent-memory-atlas/` | neoneye/agent-memory-atlas | 下载中 |
| `TRACE/` | husain34/TRACE | 下载中 |
| `mt-rag-benchmark/` | IBM/mt-rag-benchmark | 下载中 |
| `HippoRAG/` | OSU-NLP-Group/HippoRAG | 下载中 |
| `fast-graphrag/` | circlemind-ai/fast-graphrag | 下载中 |
| `mem0/` | mem0ai/mem0 | 下载中 |

**论文 PDF**（`papers/`，11/11 已落地，PDF 头校验通过）：microsoft-graphrag(2404.16130)、lightrag(2410.05779)、ket-rag(2502.09304)、synapticrag(2410.13553)、mtrag(2501.03468)、okralong(2503.02603)、hipporag(2405.14831)、zep(2501.13956)、mem0(2504.19413)、hindsight(2512.12818)、evorag(2604.15676)。
