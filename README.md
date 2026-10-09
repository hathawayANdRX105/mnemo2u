# mnemo2u 施工图 v1

**状态**：施工图 v1 待评审；骨架（`crates/mnemo-core` 类型与 trait）已建且 `cargo check` 通过；R1 未开始。
**目标**：为 coding harness 提供独立的记忆/RAG 库——graph RAG 存关联、向量 RAG 存事实、Turso 存真值，Jev 承担语义判断；公开仓、独立于 mono 开发。
**基线**：本文件；参考项目在 `todo/refs/`（nano-graphrag、LightRAG、graphrag-rs、graphiti；microsoft/graphrag 与 hindsight 待补）。
**范围**：文库 + CLI/MCP 门面；不复制 harness 的会话存储，不替代 SessionDb。

---

## 0. 命名与定位

- 名字：`mnemo2u`（crates.io 与 GitHub 全站已核，均未占用）。
- 定位：**harness 记忆层的独立实现**，与 mono 仓 `todo/context-compact` 的 FSD-CC 机制对接：
  - C01 原文真值（SessionDb）→ mnemo2u 只存派生条目 + `SourceRef`，不存原文副本。
  - C02 预算 → 检索结果计入 `BudgetAllocation`。
  - C04 `task_id` → 图的锚点。
  - C05 四工具（context_map/search/read/focus）→ mnemo2u 是 `context_search` 的语义后端。
  - C06 外发门与 `organized_through` 水位 → 写路径复用同契约。
  - C08 manifest → R6 评测复用同格式。

## 1. 架构总览（三库）

```
┌─────────────────────────────────────────────────────────┐
│ 写路径 (mnemo-index)                                      │
│  closed turn 切片 → 本地候选(零-LLM) → [高价值才过 Jev]    │
│  → 单一治理写网关（去重/冲突/tombstone 检查）→ 原子提交     │
└──────┬──────────────┬───────────────┬───────────────────┘
       ▼              ▼               ▼
┌───────────┐  ┌───────────┐  ┌────────────┐
│ turso     │  │ kuzu      │  │ lancedb    │
│ 真值+审计  │  │ 图/边      │  │ 向量 ANN   │
│ 双时态     │  │ 多跳/PPR  │  │            │
│ BM25(Tantivy)│ │ Cypher   │  │            │
└───────────┘  └───────────┘  └────────────┘
       ▲              ▲               ▲
┌──────┴──────────────┴───────────────┴───────────────────┐
│ 读路径 (mnemo-query)                                      │
│  scope 硬前滤 → 向量臂 + BM25 臂 + 图邻接臂 [+时间邻接臂]  │
│  → RRF 融合 → jev_rerank → exists_verdict                │
│  → L0 摘要先行，L2 原文按需 read                            │
└─────────────────────────────────────────────────────────┘
```

- **turso**（真值层）：facts、审计、双时态、scope 主键。Tantivy FTS（**非 SQLite FTS5 语法**，`fts_match`/`fts_score`）。
- **kuzu**（图）：边、多跳、PageRank/社区算法。上游已归档，锁 0.11.3 冻结版、MIT。
- **lancedb**（向量）：ANN 索引，活跃维护（0.40）。
- **Jev**：语义判断层（classify/compare/extract 写侧；rerank/find 读侧），外发 opt-in + 凭据预检。

## 2. 借鉴来源（逐条标注，含许可证）

| 借什么 | 来源 | 许可 | 落到 |
|---|---|---|---|
| 组件 trait 骨架（LLM/Embedding/Graph/Vector/KV 可插拔） | nano-graphrag | MIT | `mnemo-core`（已映射） |
| 双层检索（实体细节 / 主题概念）、增量更新、关系关键词标签 | LightRAG (EMNLP'25) | MIT | R3 读路径、R2 写路径 |
| 骨架式索引：只对高价值 chunk 做 LLM 抽取（成本降一个数量级） | KET-RAG (KDD'25) | 学术论文 | R2：本地候选 + Jev 选择性抽取 |
| 实体/关系/claims 图模型、社区摘要（本库不用）、local/global 检索划分 | Microsoft GraphRAG | MIT | 类型设计参考 |
| 双时态边（valid_from/valid_to 闭区间、不删只闭） | Zep/Graphiti | Apache-2.0 | Fact/Edge 字段（已进骨架） |
| trust-state 机、写网关、tombstone、evidence-before-belief、RRF 融合、scope 前置 | agent-memory-atlas | MIT | R2/R3/R5 契约 |
| 时间邻接检索（时间关联触发 + 突触传播/衰减） | SynapticRAG (2410.13553) | 论文 | R3 可选第 4 臂 |
| 分层后台记忆树、多路径检索、不阻塞主循环 | TRACE | 待核（repo 未落） | R5 后台整理形态参考 |
| 反思更新（retain/recall/reflect） | Hindsight | MIT | R5 |
| 多轮对话评测基线（110 段人写对话） | MTRAG (IBM, 2501.03468) | 数据集许可待核 | R6 |
| 零-LLM 捕获、失败恢复、审计 | atlas + LazyGraphRAG | — | R2 |

**红线**：OpenViking 为 AGPLv3——只借概念（L0/L1/L2 分层）不抄代码；所有借鉴在设计里保留出处与许可。

## 3. 技术栈

- Rust 2021 workspace（4 crates：core / store / index / query），后续加 `mnemo-cli`（门面）。
- 存储：`turso`(0.8.x) + `kuzu`(0.11.3 冻结) + `lancedb`(0.40)。
- 嵌入：`fastembed`（ONNX，bge-small 系，离线可跑）；模型名进 cache key（`hash(scope, model, text)`）。
- 异步：tokio + async-trait。
- Jev：compatible 协议客户端，固定模型版本记录在产物。

## 4. 数据契约（核心类型已在骨架落地）

- `Fact{ id, scope, kind(semantic|episodic|procedural), text, source_refs[], trust_state, valid_from, valid_to, created_at, superseded_by, revision }`
- `Edge{ src, dst, rel, weight, source_event_id }`（+ R2 增列 `timestamp, conversation_id`）
- `ScopeKey{ workspace_id, session_id?, task_id? }`：进主键、进每个索引、进 embedding cache key。
- `TrustState`：candidate / verified / rejected / stale；检索只放行 verified，candidate 带不确定性标注。
- 不变量：① 层间不共享事务，跨库提交用"先写对象、后提交指针"顺序 + 补偿（见 R2 契约测试）；② 单臂失败必须与空结果可区分；③ tombstone 在写路径检查、不进召回、不进 prompt。

## 5. 阶段路线

| 阶段 | 可交付行为 | 前置 | 验收要点 |
|---|---|---|---|
| **R1 契约层** | 骨架定型：core 类型/trait/cargo check；三库 backend 为空壳（不得注册为可用） | 无 | `cargo check` 全绿；`rrf_fuse` 单测通过 |
| **R2 写路径** | closed-turn 切片 → 本地候选（regex/关键词，零 LLM）→ 高价值 chunk 过 Jev → 治理写网关原子提交；`organized_through` 水位 | R1 | 去重/冲突/tombstone/事务失败回滚契约测试；水位幂等 |
| **R3 读路径** | scope 前滤 + 三臂（vector/BM25/graph）+ 可选时间臂 → RRF → jev_rerank → exists_verdict；L0/L2 分层返回 | R2 | 精确标识符召回、scope 负例、单臂退化可观测、融合 ablations |
| **R4 Jev 全链** | classify/compare/extract/rerank/find 全接入；外发 opt-in + 凭据预检 | R3 | 外发门测试；无 Jev 时可降级且明确失败 |
| **R5 reflect 更新** | corroborate / supersede / 来源失效重查 / tombstone；后台整理（TRACE 式分层树） | R4 | 独立来源计数防自证；旧证据失效传播 |
| **R6 评测** | MTRAG 子集 + LongMemEval 子集 + 自建 A→B→A case；manifest 冻结判据 | R5 | 冻结后跑分；质量/成本/延迟三轴报告 |

**规模策略**：第一批只索引「最近 50 轮 + 显式 pin 文档」；闭环稳定后按水位增量扩全量，**不做全量重建**。

## 6. 已定决策记录

| 决策 | 结论 | 依据 |
|---|---|---|
| 存储形态 | 三库结合（turso+kuzu+lancedb） | 用户拍板（2026-10-09），推翻早先单 SQLite 评估 |
| 图库 | kuzu 0.11.3 冻结 + `GraphStore` trait 兜底 | jev opt_d；风险已知 |
| 向量库 | lancedb（弃 qdrant-edge beta） | jev opt_d |
| 词法 | turso Tantivy FTS（非 FTS5 语法） | 官方 COMPAT.md 核实 |
| 写路径形态 | 零-LLM 捕获 + Jev 选择性增强（KET-RAG 式） | 调研 + 用户路线 |
| 命名 | mnemo2u | 用户拍板 |

## 7. 开放风险与待办

1. **kuzu 上游归档**：接受冻结版 + trait 抽象，若出事替换成本收敛在 `mnemo-store` 一个文件。需你点头。
2. **turso beta**（0.8.3-pre.1）：API 可能漂移；钉版本 + 契约测试护栏。
3. **克隆缺口**：microsoft/graphrag、hindsight 未落地（TLS 反复失败）；TGRAG 论文未核实，其数字不采信。
4. **Jev 调用成本模型**：写路径每次抽取的调用上限与缓存策略待 R4 定稿。

## 8. 参考索引

- `todo/refs/`：nano-graphrag、LightRAG、graphrag-rs、graphiti（已落地）；graphrag、hindsight（待补）。
- 论文：2404.16130(GraphRAG)、2410.05779(LightRAG)、2502.09304(KET-RAG)、2410.13553(SynapticRAG)、2501.03468(MTRAG)、2503.02603(OkraLong)、2405.14831(HippoRAG)、2501.13956(Zep)、2504.19413(Mem0)、2512.12818(Hindsight)。

## 9. 接续记录

- 已完成：core 骨架 + 三库/词法/命名决策 + refs 拷贝（4/6）。
- 未完成：R1 契约收尾、R2 起实现、graphrag/hindsight 克隆、施工图评审。
- 下一动作：你评审本文件 → 修订 → R1 开工。
