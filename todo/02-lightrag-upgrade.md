# R2 LightRAG 化增强（增量写路径 / 双关键词检索 / 级联删除 / 成本）

**状态**：待实施（依赖 R1）。
**目标**：在 R1 闭环上引入 LightRAG 的写路径与检索机制——**增量合并（不重算全局）**、关系关键词 + 双关键词检索（hl/ll）、级联删除（悬空判定/重建）、查询模式融合与成本控制。
**基准**：`todo/refs/LightRAG` @ `453dce8`（EMNLP'25；注意此副本是**深度魔改 fork**——额外带 chunker 策略 F/R/P/V、角色化 LLM、崩溃恢复账本；我们只借其核心机制，不跟随 fork 扩展）。codegraph 索引已建（29,307 节点），全部坐标已核实。
**前置**：[R1](01-nano-graphrag-port.md)验收。
**下一阶段**：[03-retrieval](03-retrieval.md)。

## 0. 与 R1 的关系（决策）

1. R1 保留 nano 语义：每次 insert **全量 drop 社区报告再重算**（参考实现明示不支持增量，`graphrag.py:329-330`）。
2. R2 引入 LightRAG 写路径：**增量合并，无全局重算**。核实依据：LightRAG 全仓 `clustering|community|leiden` 零实现（grep 佐证见 §6 偏差表附注）——它用**关系向量 + hl 关键词**做 global 检索。
3. **决策**：社区报告降级为「可选、按需触发」，`communities.mode = off | on_demand`（默认 `off`）；global 检索默认走关系向量路径。理由：社区重算与增量写路径在成本上不可共存（R1 参考实现自己承认）；R2 的核心命题是增量与成本。
4. 新增派生索引：`relationships_vdb`（lancedb）；实体向量 payload 升级为 LightRAG 形态（见 §2）——切换成本 = 一次派生库重建（R1 提交协议已保证 turso 是唯一真值）。

## 1. 映射表（源坐标 → 落点）

| LightRAG（file:line @ 453dce8） | 作用 | 我们的落点 |
|---|---|---|
| `operate.py::merge_nodes_and_edges` (3514)；调用点 `pipeline.py:5803`（每文档一次，作用域=该文档实体/边，:3645-3652） | 文档级增量合并（Phase0 锚点→实体→关系） | `src/index/merge.rs` |
| `operate.py::_merge_nodes_then_upsert` (2429)、`_merge_edges_then_upsert` (2782) | 实体/边合并（type 众数、描述去重排序、weight、keywords） | `src/index/merge.rs` |
| `operate.py::_handle_entity_relation_summary` (380) | 描述摘要（含 map-reduce 分块） | `src/index/summary.rs` |
| `operate.py::extract_entities` (3942)、`_process_extraction_result` (1516)、`_handle_single_*_extraction` (710/774) | 抽取（初抽+gleaning+解析） | `src/graph/extract.rs`（R1 扩展） |
| `operate.py::rebuild_knowledge_from_chunks` (1102) | 删除后按剩余 chunk 重建 | `src/index/purge.rs` |
| `lightrag.py::adelete_by_doc_id` (6727)、`_purge_kg_contributions` (6015)、`_purge_derived_kg_contributions` (6265) | 级联删除（悬空判定/journal 续删） | `src/index/purge.rs` |
| `operate.py::extract_keywords_only` (5112)、`get_keywords_from_query` (4975)、`kg_query` (4693) | 双关键词抽取 + 查询主入口 | `src/query/keywords.rs`、`src/query/kg.rs` |
| `operate.py::_get_node_data` (6200)、`_get_edge_data` (6529)、`_get_vector_context` (5223)、`naive_query` (6852) | local/global/mix 三路检索 | `src/query/{local,global,naive}.rs` |
| `operate.py::_build_query_context` (6075) | 预算分配 + context 渲染 | `src/query/context.rs` |
| `utils.py::use_llm_func_with_cache` (5515)、`handle_cache` (4448)、`generate_cache_key` (1090) | LLM 调用 + 缓存（含 extract/keywords 分类） | `src/llm/cache.rs` |
| `utils.py::priority_limit_async_func_call` (1193) | 并发限流（优先级+超时） | `src/core/concurrency.rs` |
| `utils.py::apply_source_ids_limit` (7244)、`merge_source_ids` (7183)、`make_relation_chunk_key` (7383) | source_id 合并与限额、追踪键 | `src/index/tracking.rs` |
| `utils.py::process_chunks_unified` (7055)、`apply_rerank_if_enabled` (6924)、`truncate_list_by_token_size` (4082) | chunk 去重→rerank→分数过滤→截断 | `src/query/fusion.rs` |
| `chunker/token_size.py::chunking_by_token_size` (133) | 切块（默认策略） | `src/graph/chunk.rs`（R1 已建） |
| `prompt.py`：`entity_extraction_*` (56/127/145)、`keywords_extraction` (484)、`summarize_entity_descriptions` (297)、`kg_query_context`/`naive_query_context` (442/469) | 提示词 | `src/graph/prompts.rs`、`src/query/prompts.rs` |

## 2. 关键常量与 payload（R2 契约）

| 项 | 值 | 源 |
|---|---|---|
| 切块 | 1200/100（env `CHUNK_SIZE`/`CHUNK_OVERLAP_SIZE`，或 addon `chunker`） | `token_size.py:133`、`lightrag.py:1471-1478` |
| gleaning | 默认 1；`MAX_EXTRACT_INPUT_TOKENS=20480` 预检跳过 | `constants.py:17,38` |
| 分隔符 | tuple `<\|#\|>`、completion `<\|COMPLETE\|>`、字段 `<SEP>` | `prompt.py:14-15`、`constants.py:49` |
| 实体类型 | 11 类：Person/Creature/Organization/Location/Event/Concept/Method/Content/Data/Artifact/NaturalObject（兜底 Other） | `prompt.py:22-36` |
| 摘要触发 | 列表长 <8 **且** 总 token<1200 → 不调 LLM 直接拼接；否则 LLM 终摘；>12000 token 先分块递归 | `constants.py:30,32,34,36`、`operate.py:437-462` |
| 边 weight | 旧权重 + 仅新 source 的权重和，再 `max(weight, 证据 chunk 数)` | `operate.py:2946-2993` |
| 实体 vdb | id=`ent-{md5(name)}`；content=`"{name}\n{description}"`；meta：entity_name/entity_type/source_id/file_path | `operate.py:2750-2756` |
| 关系 vdb | id=`rel-{md5(sorted_pair)}`（写入前删反向 id）；content=`"{keywords}\t{src}\n{tgt}\n{description}"`；meta：src_id/tgt_id/source_id/keywords/description/weight | `operate.py:3388-3400` |
| 查询默认 | mode=`mix`；top_k=40、chunk_top_k=20；max_entity_tokens=6000、max_relation_tokens=8000、max_total_tokens=30000；cosine=0.2；related_chunk_number=5；kg_chunk_pick=VECTOR；min_rerank_score=0.0；enable_rerank=true | `base.py:93,166`、`constants.py:57-67` |
| 关键词路由 | ll→entities_vdb（local/hybrid/mix）；hl→relationships_vdb（global/hybrid/mix）；两者全空且 query≥50 字 → fail_response | `operate.py:5325-5385,4759-4764` |
| 并发 | LLM 默认 `MAX_ASYNC=4`；`max_parallel_insert=3` | `constants.py:96-97` |
| 缓存键 | `{mode}:{cache_type}:{hash}`（`compute_args_hash(user+system+history,…,llm_identity)`）；extract 受 `enable_llm_cache_for_entity_extract`；chunk 回写 `llm_cache_list` 供删除 | `utils.py:1090,5515,5617-5643`、`lightrag.py:1093,1096` |
| 检索深度 | **仅 1 跳邻居展开**（无多跳图遍历） | `operate.py:6200,6529` |

## 3. 任务表

| ID | 目标（产出） | 源坐标 | 落点 |
|---|---|---|---|
| R2.T1 | 增量合并：Phase0 写 `full_entities`/`full_relations` 锚点并 flush → Phase1 实体 → Phase2 关系；作用域仅本 doc 的实体/边 | `operate.py:3514,3645-3690`、`pipeline.py:5803` | `src/index/merge.rs` |
| R2.T2 | 合并细节：type 众数、描述 (timestamp,-len) 排序+去重、weight 规则、keywords 合并、摘要三档触发 | `operate.py:2429,2782,380,437-462` | `src/index/merge.rs`、`src/index/summary.rs` |
| R2.T3 | 追踪行：`entity_chunks`/`relation_chunks` 合并 + source_id KEEP/FIFO 限额 | `utils.py:7183,7244,7383` | `src/index/tracking.rs` |
| R2.T4 | 关系关键词：抽取产 `keywords`、入图与关系向量库（payload 见 §2） | `prompt.py:73`、`operate.py:814-816,2995-3021,3388-3400` | `src/graph/extract.rs`、`src/store/lancedb.rs` |
| R2.T5 | 双关键词查询：`keywords_extraction` JSON 解析、hl/ll 路由、空关键词规则、cache_type=keywords | `operate.py:5112,4975,5325-5385`、`prompt.py:484` | `src/query/keywords.rs` |
| R2.T6 | 查询融合：chunk 去重合并→rerank（可选）→分数过滤→token 预算分配（实体/关系/总）→context 渲染；模式 local/global/hybrid/naive/mix | `operate.py:6075,5537-5630`、`utils.py:7055,6924` | `src/query/{kg,fusion,context}.rs` |
| R2.T7 | 级联删除：候选锚点→交集悬空判定→删或 `rebuild_knowledge_from_chunks`；llm_cache_list 清理；journal 断点续删 | `lightrag.py:6727,6015,6265,7162-7233`、`operate.py:1102` | `src/index/purge.rs` |
| R2.T8 | 社区开关：`communities.mode=off|on_demand`（默认 off）；on_demand 触发 R1 社区+报告并落库 | §0 决策 | `src/pipeline.rs` |
| R2.T9 | 成本：chunk 价值评分（低值零 LLM 调用）+ 抽样骨架（KET-RAG 思路）+ 模型档位 + 调用计数可观测 | `papers/ket-rag.pdf`（自研） | `src/index/cost.rs` |
| R2.T10 | 会话接入：closed-turn 适配器（`conversation_id`/时间戳入 chunk 元数据）+ `organized_through` 水位与 turso 提交**同事务** | 自研（接 C01 账本语义） | `src/index/ingest.rs` |
| R2.T11 | e2e：增量全景（插入 A→插入 B→删 A→查 B）+ 崩溃恢复 | — | `tests/` |

### 逐条判据

- **T1**：插入 1 篇新文档后：未涉及实体的行**逐字节不变**（turso 行哈希相等）；涉及实体被更新；锚点行先于图写入落库；Phase0 后崩溃→重跑幂等（无脏行）。
- **T2**：同 fixture + 同 mock 下与参考一致的合并输出；描述跨档位（<8 项且 <1200 token 不调 LLM；超限调；>12000 先分块）；权重公式三分支（新边/旧边/多证据）断言。
- **T3**：追踪行随插入/删除正确增减；source_id 限额策略（KEEP/FIFO）可配且超限行为可断言。
- **T4**：关系向量 payload 逐字段等于参考格式；同 key 重写先删反向 id；keywords 去重排序 join。
- **T5**：mock 下 hl/ll 正确解析并路由到对应 vdb（调用计数断言）；hl+ll 全空且 query≥50 字 → fail_response；缓存命中（同 mode/文本二次调用不增计数）。
- **T6**：五种模式各自检索路径断言（哪些 vdb 被查、1 跳展开次数）；rerank 开关两态；token 预算分配（entity/relation/总）边界；context 渲染与参考模板一致。
- **T7**：删文档：仅本 doc 的 chunk/向量删除；共享实体保留并重建（描述/source_id 正确）；悬空实体删除；崩溃中断后重启续删（journal）；`llm_cache_list` 对应缓存删除。
- **T8**：默认 off 时插入零社区调用（计数）；on_demand 触发后 R1 报告可查；切换开关不需要重建库。
- **T9**：低于阈值的 chunk 零 LLM 调用（计数）；抽样比例可配；模型档位（建库/查询）可分离配置。
- **T10**：会话流插入后 chunk 带 `conversation_id` 与时间戳；水位与事实同事务（崩溃点注入：提交后水位前进、提交前不动）；重放零重复。

## 4. 三库落位（R2 增量）

| 数据 | 我们 |
|---|---|
| `full_entities`/`full_relations` 锚点（key=doc_id）、`entity_chunks`/`relation_chunks` 追踪行 | turso（与事实同事务） |
| `relationships_vdb`（新增）、`entities_vdb`（payload 升级）、`chunks_vdb` | lancedb（派生，可重建） |
| 图节点/边新增字段：keywords、file_path、created_at、truncate | kuzu |
| `organized_through` 水位、会话元数据 | turso |

## 5. 功能验收（运行时可见）

- [ ] R2.FAC1 增量插入只更新受影响实体/边（无关实体行逐字节不变）。
- [ ] R2.FAC2 混合查询：具体术语（ll）与抽象主题（hl）各命中预期条目（夹具可判）。
- [ ] R2.FAC3 删文档：chunk/向量/边按预期消失；共享实体重建正确；其他文档查询不受影响。
- [ ] R2.FAC4 低价值 chunk 零 LLM 调用；阈值与抽样比例可配。
- [ ] R2.FAC5 五种查询模式可切换并各自返回结果；`mix` 默认。
- [ ] R2.FAC6 会话流插入后可按 `conversation_id`/时间过滤检索。

## 6. 测试验收（自动化）

- [ ] R2.TAC1 增量幂等：同文档重插零新增行；无关实体哈希不变。
- [ ] R2.TAC2 合并细节矩阵（type 众数/描述排序去重/weight 三分支/摘要三档）。
- [ ] R2.TAC3 删除级联全链（含共享实体重建与悬空删除）+ 崩溃续删。
- [ ] R2.TAC4 关键词：解析/路由/空规则/缓存四组。
- [ ] R2.TAC5 融合：rerank 开关、分数过滤、token 预算边界。
- [ ] R2.TAC6 缓存：`{mode}:{type}:{hash}` 键构造 + extract/keywords 开关两态 + chunk 引用回写。
- [ ] R2.TAC7 水位/幂等：崩溃点注入（提交前/后）+ 重放。
- [ ] R2.TAC8 e2e 增量全景快照（mock LLM，离线）。

## 7. 偏差记录

| 偏差 | 参考 | 我们 | 理由/影响 |
|---|---|---|---|
| 社区 | LightRAG 无社区 | 保留 R1 模块，默认 off、按需触发 | 保留 nano 能力（可作 R5/R6 语料摘要设施），但不进默认成本路径 |
| 社区核实附注 | — | grep `clustering\|community\|leiden` 仅命中 3 处无关注释（swagger JS/binding_options/routing） | 侦察证据，非推测 |
| rerank | LightRAG 走 API rerank（默认开） | 默认关（`min_rerank_score` 保留），本地模型可选 | 离线优先；R3 接多臂融合时统一决策 |
| 追踪/摘要阈值的实现形态 | fork 版含 truncation 账本等扩展 | 只保留核心（限额+摘要三档） | 不跟随 fork 扩展，避免过度设计 |
| 实体 vdb payload | `"{name}\n{description}"` | 采纳（切换 = 派生库重建） | 与 R1 payload 不同，重建路径已具备 |

## 8. 接续记录

- 已完成：无（依赖 R1）。
- 未完成：R2.T1–T11。
- 下一动作：R1 验收后，先落 T1+T3（增量合并与追踪行，配套崩溃点测试），再 T4–T6（检索链路）。
