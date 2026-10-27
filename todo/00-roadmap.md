# mnemo2u 路线图（施工图 v1）

**状态**：v1 施工中。R1（nano-graphrag 复刻）已实现并验收（43 用例 CI 全绿；遗留 4 项——报告双端 golden/FAC6/崩溃点注入/实体向量重建——已收口 `388b219`，见 01 §9）。R2（LightRAG 增强）已实施并完成 jev 审计闭环（PR #1–#3 squash 合并；FAC1–6/TAC1–8 全勾，FAC6 已随 `388b219` 落地，B 系列为登记偏差，见 02 §7/§8）。R3（读路径硬化）已实施并验收（PR #4/#5 squash 合并 `44c160b`/`5d6c74c`，2026-10-12；T1–T7 与 AC1–AC5 勾账，jev 两轮 11+14 verified / 0 contradicted，canon 全量 diff ALL PASS，见 03 接续记录）。R4（Judge 全链）已实施（PR #7 squash 合并 `46a37cb`：JudgeClient trait + 统一执行外壳 + 外发门 + jev-compatible 首个后端 + 缓存 + 降级语义）。写网关（closed turns → facts）与持久化 turso `FactStore` 已合并（PR #11 squash `172c15d`，2026-10-15）——R4 判断接入**写侧**的接线，`judge_write_candidates` 由此有了调用方；6-agent 审查的两个 blocker（复合主键含 NULL 列致 `put` 对任何缺失可选分量的 scope 不幂等、规模轴塌成 4 事实）与四项修复已勾账。R6 六任务全落并已收尾：T1/T4/T5/T6 随 PR #11；T2/T3（MTRAG/LongMemEval 外部集，许可已核 Apache-2.0/MIT）、RAG pipeline conversation/时间过滤、R1 四遗留均随 PR #12 squash `388b219`（2026-10-16）合并，分支 `feat/r6-and-gaps` 已删。R5（反思更新）已实施并合并：四片 PR #14–#17 squash `911a6c6`/`25c6e8a`/`04b5332`/`53ff0b2`（2026-10-16；含 valid_to + corroborate、tombstone、invalidate、stale 读时派生，契约 v2 见 05）。2026-10-16 真实端点冒烟通过：本地 SystemOne 三操作 Ok、CLI 全链（insert/query/rebuild）Ok；PR #18 `6cf60b9` 修复 wire tag 大小写与默认 User-Agent 两个缺陷。R1 golden parity 覆盖补强（自动子社区切换 + 子报告截断/过滤两 case，mutation 探针验证）已合并（PR #19 `c023470`，2026-10-17）。R7（自我诊断）S1+S2+S3+S3.5+S3.5.1+S3.5.2+S3.5.3+S3.5.4 均已合并：S1（PR #20 `bd2c621`，2026-10-17）`diag::attribute` 读路径失败归因 + `diag::health` 臂健康行；S2（PR #21 `d42251f`，2026-10-17）`tune::KnobRegistry` 12 旋钮 + `knobs_for(stage)` 按归因选旋钮的机械保证 + scoreboard 基线（质量/成本精确环比，延迟只记录）+ `MemoryRecall::recall_with` 注入缝（契约见 07 文档）；S3（PR #22 `ef93ba8`，2026-10-18）`tune::Search` 搜索闭环 + 四条接受规则 + `run_case_with` 缝 + 13 测；S3.5（PR #23 `bde4d77`，2026-10-18）`Case::edges` 关系声明 + `graph_hop_case` 未饱和调参集 + 7 测——**搜索第一次真正接受方向**（`graph_hops 2→3`，gain 1 > 地板 0.5，棘轮生效，消融数据见 07 文档）；S3.5.1（PR #24 `969877b`，2026-10-18）holdout 扩到 6 query；S3.5.2（PR #25 `62ab143`，2026-10-18）补 `KnobRegistry::set` 错误路径测试——**jev 三轮评审（#22/#23+#24）0 contradicted / 0 unsupported，唯一 `test_gap` 缺口由此闭环**；S3.5.3（PR #27 `7b418eb`，2026-10-18）修 `set` 不校验值种类的缺陷（`F64` 可写进行 `u8`，直到 `recall_knobs` 才失败）；S3.5.4（#28 `9c7cf79` 无值行断言扩到整个类、#29 `812f3c4` 取名 `same_kind`、#30 `1e86719` 钉住 `admits` 的承重不变量）。**下一会话开工点：R7 S3.6——给 `Case` 的边加更多关系类型（`DependsOn`/`MentionsFile` 等）以解锁 `rel_support` 等其余旋钮；S3.5 只让 `graph_hops` 一条路可走。任务书 [handoff/r7-s3-set](handoff/r7-s3-set.md)（含阻塞调查、三个选项与两轮 CI 返工教训）。之后是 S4 语义档 + 飞轮（design/08 §五）。**  
**目标**：为 coding harness 提供独立的记忆/RAG 库——graph RAG 存关联、向量 RAG 存事实、Turso 存真值，Jev 承担语义判断；公开仓、独立开发。
**基线**：本文件 + 分阶段文档（01–07）；参考项目在 `todo/refs/`。

## 文档索引

| 文档 | 阶段 | 内容 |
|---|---|---|
| [01-nano-graphrag-port](01-nano-graphrag-port.md) | R1 | **复刻 nano-graphrag**：切块→抽取→合并→建图→社区→三模式查询 + 三库提交协议 |
| [02-lightrag-upgrade](02-lightrag-upgrade.md) | R2 | **LightRAG 增强**：增量更新/双层检索/关键词标签/合并升级/选择性删除/成本控制 |
| [03-retrieval](03-retrieval.md) | R3 | 读路径硬化：scope 前滤→多臂→RRF→rerank→分层返回 |
| [04-jev-integration](04-jev-integration.md) | R4 | Jev 全链接入与外发门 |
| [05-reflect](05-reflect.md) | R5 | 反思更新：corroborate/supersede/失效传播 |
| [06-evaluation](06-evaluation.md) | R6 | 评测：MTRAG/LongMemEval/自建 case + manifest |
| [07-self-diagnosis](07-self-diagnosis.md) | R7 | 自我诊断：读路径失败归因 + 臂健康行（S1 `bd2c621`）+ 旋钮登记/scoreboard 基线（S2 `d42251f`）+ 搜索闭环与接受规则（S3 `ef93ba8`）；S3.5 `Case::edges` + 未饱和调参集，搜索首次接受方向（`bde4d77`/`969877b`/`62ab143`/`7b418eb`/`9c7cf79`/`812f3c4`/`1e86719`，jev 三轮 0 contradicted）；S3.6 加更多边类型、S4 语义档 + 飞轮待开工（design/08） |
| [refs/](refs/) | — | 参考项目克隆与论文 PDF（见 §10） |

阅读顺序：新会话先读本文件 → 当前阶段文档 → 其前置阶段的接续记录。不依赖聊天口头结论。
**开发形态**：单 crate（`src/` 子目录划分大功能：`core` / `store` / `llm` / `graph` / `index` / `query` / `diag`），需要时再拆 workspace。

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
        ├─→ redb    图/边邻接 + 内存邻接索引              ← 派生，可重建
        └─→ lancedb 向量 ANN（0.40）                      ← 派生，可重建

读路径 (mnemo-query)
  scope 硬前滤 → 向量臂 + BM25 臂 + 图邻接臂 [+时间臂]
  → RRF 融合 → jev_rerank → exists_verdict
  → L0 摘要先行，L2 原文按需 read（SourceRef 回 harness）
```

**可重建性是核心不变量**：turso 是唯一真值；lancedb（向量索引）与 redb 邻接表（图索引）都是派生，任何时候可以从真值全量重建。索引不共享事务，跨索引一致性靠提交协议（见 01）。

## 3. 借鉴来源（逐条标注，含许可证）

| 借什么 | 来源 | 许可 | 本地副本（todo/refs/） | 落到 |
|---|---|---|---|---|
| 组件 trait 骨架（LLM/Embedding/Graph/Vector/KV 可插拔） | nano-graphrag | MIT | `nano-graphrag/` @ acb35c0 | `src/core`（已映射）；R1 全面复刻 |
| 增量合并（无全局重算）、关系向量 + hl/ll 双关键词检索、级联删除与悬空判定、追踪行/限额 | LightRAG (EMNLP'25) | MIT | `LightRAG/` @ 453dce8（深度魔改 fork，仅借核心机制） | R2 |
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
- **图存储**：**redb**（独立文件 + 内存邻接索引）。R1 曾用 turso 邻接表，2026-10-21 迁到 redb：图调用不再与 KV/事实写抢 turso 单连接的锁，多跳遍历从逐跳 SQL 往返变成内存指针跳转。kuzu 0.11.3 静态链与 turso_core 的 simsimd 冲突不可解（cxxbridge shim 未链入）+ 上游仓库已归档，**永久劝退**；三次偏离的完整记录见 `01 §8.5` 与 `01 §8.7`。
- **图算法**：`leiden-rs` 0.8（层次社区检测，R1 社区）、`petgraph` 0.8（内存图辅助/算法）；PPR 自实现（参考 `refs/fast-graphrag`，R3）。
- **向量库**：`lancedb` 0.40（嵌入式 ANN）。
- **真值库**：`turso` 0.8.x（beta；SQLite 兼容、原生 async、Tantivy FTS BM25）。
- **嵌入**：`fastembed` 7.x（ONNX 本地，bge-small 系；离线；模型名进缓存键）。
- **分词/计数**：**`tiktoken-rs` 0.12.1**（与参考实现同 tokenizer，cl100k/o200k——切块与截断的等价基准）；`jieba-rs` 0.11 仅用于中文 FTS 辅助。
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
| R2 | **LightRAG 增强**：增量合并（无全局重算）、关系向量+hl/ll 双关键词检索、级联删除、追踪行、成本 | 功能验收 FAC1–6；测试验收 TAC1–8 |
| R3 | 读路径硬化：前滤→多臂→融合→rerank→分层返回 | 精确召回、scope 负例、降级可观测 |
| R4 | Jev 全链 + 外发门 | 未 opt-in 零外发；无 Jev 显式降级 |
| R5 | 反思更新（corroborate/supersede/失效传播） | 纠正抗重建、独立来源计数、级联失效；契约 v2 已删「后台整理」片（见 05） |
| R6 | 评测体系 | 冻结判据后跑分；三轴报告 |

**规模策略**：第一批只索引「最近 50 轮会话 + 显式 pin 文档」；闭环稳定后按水位增量扩全量，**不做全量重建**。

## 7. 已定决策记录

| 决策 | 结论 | 依据 |
|---|---|---|
| 存储形态 | turso（真值）+ redb（图）+ lancedb（向量） | 用户拍板（2026-10-21 改图库）；图引擎偏离见 01 §8.5/§8.7 |
| 图库 | redb 邻接 + 内存索引；kuzu 永久出局（归档 + 符号冲突） | 链接期证据 + 上游归档；01 §8.5/§8.7 |
| 向量库 | lancedb（弃 qdrant-edge beta） | jev opt_d |
| 词法 | turso Tantivy FTS（非 FTS5 语法：`fts_match`/`fts_score`） | 官方 COMPAT.md 核实 |
| 写路径形态 | 零-LLM 捕获 + Jev 选择性增强（KET-RAG 式） | 调研 + 用户路线 |
| 社区策略 | R1 保留 nano 语义（全量重算）；R2 起默认 `off`、按需触发 | LightRAG 全仓无社区（grep 佐证）；增量与社区重算成本不可共存 |
| 命名 | mnemo2u | 用户拍板 |

## 8. 开放风险与待办

1. ~~kuzu 上游归档~~：已处置——R1 不引入 kuzu（01 §kuzu 偏离），替换成本收敛在 `src/store/turso_graph.rs` 一个文件。
2. **turso beta**（0.8.3-pre.1）：API 可能漂移；钉版本 + 契约测试护栏。
3. **参考库**：论文 11/11、克隆 12/12 全部落地（见 §10，均为浅克隆）。TGRAG 论文未核实，其数字不采信。
4. **Jev 调用成本模型**：写路径每次抽取的调用上限与缓存策略在 R4 定稿。
5. **TRACE 与 MTRAG 许可**：使用前核实。

## 9. 接续记录

- 已完成：R1–R4 全阶段 + 写网关/持久化 turso FactStore（PR #11）+ R6 自建评测切片（manifest + 自建 case + 三轴报告 + CI gate）。各阶段验收明细见对应文档的接续记录。
- 未完成：R5（反思更新，见 05）；R6 的 MTRAG/LongMemEval（许可待核，见 06）；R1 报告路径双端字节实测（01 §9）。
- 下一动作：R5（[05-reflect](05-reflect.md)）。

## 10. 参考库索引（todo/refs/）

**项目克隆**：

| 目录 | 仓库 | 状态 |
|---|---|---|
| `nano-graphrag/` | gusye1234/nano-graphrag | ✅ acb35c0 |
| `LightRAG/` | HKUDS/LightRAG | ✅ 453dce8 |
| `graphrag/` | microsoft/graphrag | ✅ 5faaaf4 |
| `graphrag-rs/` | automataIA/graphrag-rs | ✅ a7222c1 |
| `graphiti/` | getzep/graphiti | ✅ 1026ae7 |
| `hindsight/` | vectorize-io/hindsight | ✅ 6d8b096 |
| `agent-memory-atlas/` | neoneye/agent-memory-atlas | ✅ 9d99eaa |
| `TRACE/` | husain34/TRACE | ✅ 8f764be |
| `mt-rag-benchmark/` | IBM/mt-rag-benchmark | ✅ 53fef6f（稀疏：scripts/README） |
| `HippoRAG/` | OSU-NLP-Group/HippoRAG | ✅ d5c8329 |
| `fast-graphrag/` | circlemind-ai/fast-graphrag | ✅ 23b3a1b |
| `mem0/` | mem0ai/mem0 | ✅ b7ad69a |

说明：全部为 `--depth 1` 浅克隆（读参考代码足够）；`mt-rag-benchmark` 数据集（corpora 206MB）未拉取，用 `blob:none` + cone 稀疏检出，需要哪个文件单独 `git show` 即懒加载。

**论文 PDF**（`papers/`，11/11 已落地，PDF 头校验通过）：microsoft-graphrag(2404.16130)、lightrag(2410.05779)、ket-rag(2502.09304)、synapticrag(2410.13553)、mtrag(2501.03468)、okralong(2503.02603)、hipporag(2405.14831)、zep(2501.13956)、mem0(2504.19413)、hindsight(2512.12818)、evorag(2604.15676)。
