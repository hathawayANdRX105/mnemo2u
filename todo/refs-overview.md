# mnemo2u 参考项目概览（todo/refs/）

本目录下的 `refs/` 为对外参考项目的**浅克隆**（`--depth 1`），仅用于读代码、读设计，不并入 mnemo2u 源码，不作为依赖。各自许可证以其仓库为准，借用时需标注出处并遵守对应协议。

## 红线（来自 00-roadmap.md §3）

- **OpenViking 为 AGPLv3**——只借 L0/L1/L2 分层概念，**不抄代码**。
- 所有借鉴保留出处与许可证；落到代码后在对应阶段文档标注。

## 按借用用途分类

| 用途 | 项目 | 来源 | 许可 | 本地路径 | 主要借用点 | 落到阶段 |
|---|---|---|---|---|---|---|
| 图模型与检索 | nano-graphrag | gusye1234/nano-graphrag | MIT | `refs/nano-graphrag/` | 组件 trait 骨架（LLM/Embedding/Graph/Vector/KV 可插拔） | R1 |
| 图模型与检索 | LightRAG | HKUDS/LightRAG | MIT | `refs/LightRAG/` | 增量合并（无全局重算）、关系向量 + hl/ll 双关键词检索、级联删除与悬空判定 | R2 |
| 图模型与检索 | graphrag | microsoft/graphrag | MIT | `refs/graphrag/` | 实体/关系/claims 图模型、local/global 检索划分 | R1 对齐参考 |
| 图模型与检索 | graphiti | getzep/graphiti | Apache-2.0 | `refs/graphiti/` | 双时态边（闭区间不删） | `Fact`/`Edge` 字段 |
| 图模型与检索 | fast-graphrag | circlemind-ai/fast-graphrag | — | `refs/fast-graphrag/` | PPR 图召回实现参考 | R3 |
| 图模型与检索 | graphrag-rs | automataIA/graphrag-rs | MIT | `refs/graphrag-rs/` | 7 阶段管线全景、配置驱动 | R1 参考 |
| 记忆层与写网关 | agent-memory-atlas | neoneye/agent-memory-atlas | MIT | `refs/agent-memory-atlas/` | trust-state / 写网关 / tombstone / evidence-before-belief / RRF / scope 前置 | R2/R3/R5 |
| 记忆层与写网关 | hindsight | vectorize-io/hindsight | MIT | `refs/hindsight/` | retain/recall/reflect | R5 |
| 记忆层与写网关 | mem0 | mem0ai/mem0 | 各自 | `refs/mem0/` | 记忆层全景参考（实现对比） | R5 参考 |
| 记忆层与写网关 | TRACE | husain34/TRACE | 待核 | `refs/TRACE/` | 分层后台记忆树、多路径检索 | R5 形态参考 |
| 评测基线 | mt-rag-benchmark | IBM/mt-rag-benchmark | 待核 | `refs/mt-rag-benchmark/` | 多轮评测基线（110 段） | R6 |
| 评测基线 | HippoRAG | OSU-NLP-Group/HippoRAG | — | `refs/HippoRAG/` | — | — |
| 工程参考平台 | SuperMew | （本地克隆） | — | `refs/SuperMew/` | 持久化 Thread/Run/Event、RAG 评测、HITL、模型控制面、Skill/Tool Registry | 参考 |

## 论文 PDF（refs/papers/）

| 文件 | 对应项目/用途 |
|---|---|
| `microsoft-graphrag.pdf` | GraphRAG (2404.16130) |
| `lightrag.pdf` | LightRAG (2410.05779) |
| `ket-rag.pdf` | KET-RAG 骨架式索引 (2502.09304) |
| `synapticrag.pdf` | 时间邻接检索 (2410.13553) |
| `mtrag.pdf` | MTRAG (2501.03468) |
| `okralong.pdf` | OkraLong (2503.02603) |
| `hipporag.pdf` | HippoRAG (2405.14831) |
| `zep.pdf` | Zep/Graphiti 双时态 (2501.13956) |
| `mem0.pdf` | Mem0 (2504.19413) |
| `hindsight.pdf` | Hindsight (2512.12818) |
| `evorag.pdf` | EvoRAG (2604.15676) |

## 使用提醒

- `mt-rag-benchmark` 做了稀疏检出（未拉取 206 MB 数据集 corpus），按需 `git show` 懒加载。
- `mt-rag-benchmark`、`TRACE`、`mem0`、`HippoRAG`、`fast-graphrag` 许可证**待核实**，用于落地代码前需确认。
- `refs/` 内每个子目录自带独立 `.git`（独立仓库），与 mnemo2u 无 git 关联，不构成子模块。
