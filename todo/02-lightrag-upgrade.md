# R2 LightRAG 化增强（增量写路径 / 双关键词检索 / 级联删除 / 成本）

**状态**：已实施并完成审计闭环（PR #1 squash 合并 `8201bdd`；PR #2 squash 合并 `6589270` 闭环 2026-10-11 jev 审计的代码缺口与 CI 抓出的两处 src 真缺陷；PR #3 squash 合并 `a08c768` 落 FAC5 维护者裁决）。2026-10-11 jev 完成度审计（47 条判定：33 verified / 14 contradicted / 0 说不清）：可代码闭环的 contradicted 全部经 PR #2 修复并复评 **15/15 verified**（`jev_verify` model `jev-1.13.0`，证据坐标内联 §5/§6/§7/§8）；FAC5 已裁决落地、FAC6 conversation 维已随 R3.T1 落地（PR #4），B 系列为登记偏差（§7/§8）。
**目标**：在 R1 闭环上引入 LightRAG 的写路径与检索机制——**增量合并（不重算全局）**、关系关键词 + 双关键词检索（hl/ll）、级联删除（悬空判定/重建）、查询模式融合与成本控制。
**基准**：`todo/refs/LightRAG` @ `453dce8`（EMNLP'25；注意此副本是**深度魔改 fork**——额外带 chunker 策略 F/R/P/V、角色化 LLM、崩溃恢复账本；我们只借其核心机制，不跟随 fork 扩展）。codegraph 索引已建（29,307 节点），全部坐标已核实。
**前置**：[R1](01-nano-graphrag-port.md)验收。
**下一阶段**：[03-retrieval](03-retrieval.md)。

## 0. 与 R1 的关系（决策）

1. R1 保留 nano 语义：每次 insert **全量 drop 社区报告再重算**（参考实现明示不支持增量，`graphrag.py:329-330`）。
2. R2 引入 LightRAG 写路径：**增量合并，无全局重算**。核实依据：LightRAG 全仓 `clustering|community|leiden` 零实现（grep 佐证见 §6 偏差表附注）——它用**关系向量 + hl 关键词**做 global 检索。
3. **决策**：社区报告降级为「可选、按需触发」，`communities.mode = off | on_demand`（默认 `off`）；global 检索默认走关系向量 + hl 关键词路径。理由：LightRAG 从未实现社区检测（`clustering|leiden|community_report` 全仓零命中，见 §7 附注），其 global 检索即 hl 关键词 → `relationships_vdb`；而 nano-GraphRAG 的社区报告是全量重算产物（`graphrag.py:329-330`），与 R2 要移植的增量写在成本上冲突。
4. 新增派生索引：`relationships_vdb`（lancedb）；实体向量 payload 升级为 LightRAG 形态（见 §2）——切换成本 = 一次派生库重建（R1 提交协议已保证 turso 是唯一真值）。

## 1. 映射表（源坐标 → 落点）

| LightRAG（file:line @ 453dce8） | 作用 | 我们的落点 |
|---|---|---|
| `operate.py::merge_nodes_and_edges` (3514)；调用点 `pipeline.py:5803`（每文档一次，作用域=该文档实体/边，:3645-3652） | 文档级增量合并（Phase0 锚点→实体→关系） | `src/index/merge.rs` |
| `operate.py::_merge_nodes_then_upsert` (2429)、`_merge_edges_then_upsert` (2782) | 实体/边合并（type 众数、描述去重排序、weight、keywords） | `src/index/merge.rs` |
| `operate.py::_handle_entity_relation_summary` (372) | 描述摘要（含 map-reduce 分块） | `src/index/summary.rs` |
| `operate.py::extract_entities` (3942)、`_process_extraction_result` (1508)、`_handle_single_*_extraction` (710/774) | 抽取（初抽+gleaning+解析） | `src/graph/extract.rs`（R1 扩展） |
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
| 取值口径 | 已钉死的实现值（`src/core/rag.rs:77-85,150-173`）：mode=**Global**（nano 口径，参考的 mix 未采纳→§7）、top_k=**20**（参考 40）、chunk_top_k=20、max_entity/relation/total=**6000/8000/30000**、cosine=0.2、min_rerank_score=0.0、enable_rerank=**false**（离线优先）；并发维持 R1 口径 **16**（参考 MAX_ASYNC=4 / max_parallel_insert=3 未移植）；`related_chunk_number=5`、`kg_chunk_pick=VECTOR` 未移植 | 实现期裁决（2026-10-11 审计回填） |

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
| R2.T11 | e2e：增量全景（插入 A→插入 B→拆 A→查 B）+ 崩溃恢复 | — | `tests/` |

> T9/T10 为自研件（源坐标列已标「自研」）。维护者 2026-10-10 拍板**一次性完整交付**，T9/T10 随本阶段实现；实际口径收敛见 §7 偏差表（T9=评分+cap+台账，T10=提交幂等+水位，模型档位/采样/查询侧过滤未做）。

### 逐条判据

- **T1**：插入 1 篇新文档后：未涉及实体的行**逐字节不变**（turso 行哈希相等）；涉及实体被更新；锚点行先于图写入落库；Phase0 后崩溃→重跑幂等（无脏行）。
- **T2**：同 fixture + 同 mock 下与参考一致的合并输出；描述跨档位（<8 项且 <1200 token 不调 LLM；超限调；>12000 先分块）；权重公式三分支（新边/旧边/多证据）断言。
- **T3**：追踪行随插入/删除正确增减；source_id 限额策略（KEEP/FIFO）可配且超限行为可断言。
- **T4**：关系向量 payload 逐字段等于参考格式；同 key 重写先删反向 id；keywords 去重排序 join。
- **T5**：mock 下 hl/ll 正确解析并路由到对应 vdb（调用计数断言）；hl+ll 全空且 query≥50 字 → fail_response；缓存命中（同 mode/文本二次调用不增计数）。
- **T6**：五种模式各自检索路径断言（哪些 vdb 被查、1 跳展开次数）；rerank 开关两态；token 预算分配（entity/relation/总）边界；context 渲染与参考模板一致。
- **T7**：删文档：仅本 doc 的 chunk/向量删除；共享实体保留并按剩余来源的缓存抽取重建（描述/source_id/file_path 正确）；悬空实体删除；崩溃中断后重跑续删收敛（裁决=不引入 journal，幂等序收敛，§8）；`llm_cache_list` 对应缓存删除。
- **T8**：默认 off 时插入零社区调用（计数）；on_demand 触发后 R1 报告可查；切换开关不需要重建库。
- **T9**：低于阈值的 chunk 零 LLM 调用（计数）；抽样比例可配；模型档位（建库/查询）可分离配置。
- **T10**：会话流插入后 chunk 带 `conversation_id` 与时间戳；水位与事实同事务（崩溃点注入：提交后水位前进、提交前不动）；重放零重复。

## 3.5 缺口契约（原文档遗漏，实现前必须钉死）

1. **图 schema 迁移**：R2 给 turso 图节点/边加 `keywords`、`file_path`、`created_at`（`operate.py:1791,2232,2330,3172`）。R1 已存在行需一次性迁移（旧行默认值 `keywords=''`、`file_path='unknown_source'`、`created_at=0`），迁移完成前拒绝混合 schema 写入；失败整体回滚。
2. **Repair queue 覆盖扩展**：R1 队列只覆盖既有派生库。R2 新增 `relationships_vdb` 与 `entity_chunks`/`relation_chunks` 追踪行后，队列目标与 `rebuild_chunk_vectors` 的输入域必须同步扩展，否则崩溃恢复后出现「图已合并且溯源缺失」的中间态。
3. **T8 on_demand 契约**：触发点 = 显式调用（查询侧/管理侧入口），不在插入路径自动跑；结果落 R1 既有社区/报告表；触发期间 LLM 计数单独归组；未触发时零社区开销。
4. **T9/T10 为自研件**（无参考坐标）：成本采样（KET-RAG 思路）与会话水位接入都需要独立设计评审；建议在 R2 核心件（T1–T8、T11）落地并有真实消费方后再启动，不与移植件混在同一评审轮。

## 4. 三库落位（R2 增量）

| 数据 | 我们 |
|---|---|
| `full_entities`/`full_relations` 锚点（key=doc_id）、`entity_chunks`/`relation_chunks` 追踪行 | turso（与事实同事务） |
| `relationships_vdb`（新增）、`entities_vdb`（payload 升级）、`chunks_vdb` | lancedb（派生，可重建） |
| 图节点/边新增字段：keywords、file_path、created_at（实体与关系皆写） | turso（图邻接表，需 schema 迁移） |
| vdb `truncate` 截断标记（`_truncate_vdb_content` 的返回） | lancedb（payload/meta 侧，不进图） |
| `organized_through` 水位、会话元数据 | turso |

## 5. 功能验收（运行时可见）

- [x] R2.FAC1 增量插入只更新受影响实体/边：增量路径（锚点先行 `pipeline.rs:249`、文档作用域合并），「无关实体行逐字节不变」已断言（`tests/index_idempotency.rs:420`，含正对照）。
- [x] R2.FAC2 混合查询路由：local 实体臂 / global 关系臂 / naive chunk 三臂命中断言（`tests/pipeline_e2e.rs:215`，jev verified 0.97）。
- [x] R2.FAC3 删文档不影响他档：共享实体按剩余来源的缓存抽取重建 description/source_id/file_path（`a885169`，`operate.py:1102` 同构）、独占删除、no-op（`tests/index_purge.rs:201/:329`；全景 `tests/index_incremental_e2e.rs:154`）。
- [x] R2.FAC4 低价值 chunk 零 LLM 且阈值/cap 可配（`tests/index_cost.rs` 5 用例）。
- [x] R2.FAC5 五模式可切换并返回结果（`tests/pipeline_e2e.rs`）；默认已按维护者 2026-10-11 裁决对齐参考 = **Mix + top_k 40**（`rag.rs:153/:157`，PR #3）。
- [x] R2.FAC6 按 `conversation_id`/时间过滤检索：memory 层 session 维随 R3.T1 落地（PR #4：`ScopeKey.session_id` + `ScopeKey::admits` 臂内前滤，`tests/scope_prefilter.rs`/`tests/lexical_recall.rs` 否定测试）；RAG pipeline `QueryParam` 过滤已落地（`388b219`：`conversation_id`/`TimeWindow` 字段 + kg/naive 取回后过滤 + `answer_cache_inputs` 纳键 + `tests/query_param_filters.rs` 5 用例）。登记口径：载体=向量行 `VectorHit::meta`（`META_CONVERSATION_ID`/`META_TIMESTAMP`，`ingest.rs:228-239`），graph 行仅 `created_at` 有时间无会话——缺载体行在带过滤时宁缺勿假丢弃（回执 §2 载体问题逐字段核过）。

## 6. 测试验收（自动化）

- [x] R2.TAC1 增量幂等：同文档重插逐字节 no-op（六 KV 表+图+三路向量+LLM 计数+repair pending 全快照）与增量插入无关行逐字节不变含正对照（`tests/index_idempotency.rs:345/:420`）。
- [x] R2.TAC2 合并细节矩阵：`merge_matches_python_reference` + R2 重生成 golden（权重三分支/描述 union 排序/类型众数/摘要三档）。
- [x] R2.TAC3 删除级联：共享重建/悬空/no-op 已测（`tests/index_purge.rs:201/:329`）；journal 断点续删裁决=不引入——幂等续删回归 `tests/crash_recovery.rs:1039`（中断删除重跑与一次成功逐字节收敛，§8）。
- [x] R2.TAC4 关键词：解析 5 用例 + cache_type 分离 + 全空 fail + 解析失败先重试（上限 3，成功覆盖坏缓存行、耗尽才 warn 回退空表，`keywords.rs:41,79-97`，`tests/query_keywords.rs` 3 用例）。
- [x] R2.TAC5 融合：dedup/chunk_top_k/token 预算/零预算/rerank 开关两态（`tests/query_fusion.rs` 5 用例）。
- [x] R2.TAC6 缓存：key golden 保留；用途开关 `enable_llm_cache_for_extract`/`enable_llm_cache` 默认 true、两态独立 4 测试（`cache.rs:21-80`，`tests/llm_cache.rs`）+ `llm_cache_list` 回写（`extract.rs` chunk_cache_keys → `pipeline.rs:425`）+ 删除清理（`purge.rs:92-105/:185/:577-606`，`tests/index_cache_bookkeeping.rs`）。
- [x] R2.TAC7 水位幂等：重放零新增/幂等/水位先行（`tests/index_ingest.rs`）+ 崩溃点注入：真值 KV 失败无半提交行、派生向量失败进修复队列重放、insert/delete 中断重跑收敛（`tests/crash_recovery.rs` 4 场景，注入器全在测试侧、零 src 侵入）。
- [x] R2.TAC8 e2e 增量全景：插 A→插 B（共享实体）→删 A→三模式查 B——B 证据在场、A exclusive 证据全灭（`tests/index_incremental_e2e.rs:154`）。

## 7. 偏差记录

| 偏差 | 参考 | 我们 | 理由/影响 |
|---|---|---|---|
| 社区 | LightRAG 无社区 | 保留 R1 模块，默认 off、按需触发 | 保留 nano 能力（可作 R5/R6 语料摘要设施），但不进默认成本路径 |
| 社区核实附注 | — | `lightrag/**` 源码内 `clustering\|leiden\|community_report` 零命中；命中的是无关注释（`lightrag/parser/routing.py:609` 语义聚类注释、`binding_options.py`、`swagger-ui-bundle.js`、README 的 Discord/community contributions 字样） | 侦察证据，非推测 |
| rerank | LightRAG 走 API rerank（默认开） | 默认关（`min_rerank_score` 保留），本地模型可选 | 离线优先；R3 接多臂融合时统一决策 |
| 追踪/摘要阈值的实现形态 | fork 版含 truncation 账本等扩展 | 只保留核心（限额+摘要三档） | 不跟随 fork 扩展，避免过度设计 |
| 实体 vdb payload | `"{name}\n{description}"` | 采纳（切换 = 派生库重建） | 与 R1 payload 不同，重建路径已具备 |
| 查询默认 | mode=mix、top_k=40 | **已对齐**：Mix、top_k=40（`rag.rs:153/:157`，维护者 2026-10-11 裁决，PR #3） | 曾维持 nano 口径；五模式切换已测（FAC5） |
| rerank | API rerank 默认开 | trait seam + 开关（两态有测试），**无内置实现**，开启且无 reranker 时跳过（`fusion.rs:44-45`） | 离线优先；接 R3 多臂时补 |
| context 渲染 | operate.py 模板 | 手写移植，**无逐字节 golden**（parity fixture 不含渲染） | R2 未开渲染 parity 承诺 |
| 删除 journal / 缓存清理 | journal 续删 + `llm_cache_list` 级联清理 | journal 裁决=不引入：幂等序续删（chunk 行先删、anchor 行最后删作完成标记，`purge.rs:186-189`），中断重跑收敛已测（`tests/crash_recovery.rs:1039`）；缓存清理已做（正向 `llm_cache_list` 引用 + legacy 反扫） | A1/A9 已闭环 |
| T9 口径 | 抽样骨架 + 模型档位 | 评分阈值 + per-run cap + CostLedger | 采样比例/档位分离未实现（审计 B12） |
| T10 口径 | 水位与事实**同事务** + 会话过滤 | `commit_turns` 幂等 + 水位先于图写（顺序保证，非同一 SQL 事务）；查询侧过滤未做 | 审计 B13/B14；同事务受 turso 单写接口限制 |
| 缓存键与开关 | `{mode}:{cache_type}:{hash}` + extract/keywords 独立开关 + chunk 引用回写 | 键 `md5(purpose|model|hash|params|mode)`（purpose 前缀 ≡ cache_type，golden 保留）；双开关 `enable_llm_cache`/`enable_llm_cache_for_extract` 默认 true；`llm_cache_list` 回写与删除清理已做 | A2/A6 已闭环 |
| §3.5-1 迁移 | 一次性列迁移 + 混合 schema 拒写 | **JSON props 透传 + 读侧默认值**（`types.rs:34-42`、`merge.rs:48`），无迁移代码 | 契约目的（旧行可读、默认值确定）达成，手段替换——本条即登记 |
| source_ids 限额口径 | token/条数口径 | **字节口径** 50 规则 | A4（对齐 fork 实测行为） |
| 参考特性未移植 | related_chunk_number、kg_chunk_pick、file_path≤75 校验、实体名≤256 校验、max_parallel_insert | 未移植 | 差异报告 B 系列；校验类缺口列入待办 |

## 8. 接续记录

- 已完成：R2.T1–T11 + §3.5 全部落地（PR #1 合并 `8201bdd`）。2026-10-11 jev 完成度审计（`jev_verify`，model `jev-1.13.0`，三批 47 条）→ **PR #2 闭环**（squash `6589270`）：§8 旧缺口 1–5（N1 测试、TAC8 全景、崩溃注入 harness、N2 重试、缓存开关+回写）全部落地；CI 首跑的两个回归测试又抓到两处 src 真缺陷，同 PR 修复——`4ad987a` scope anchor 批作用域污染（整批 anchor 写给批内每个 doc，无关实体被拉进删除候选；改为按 doc 过滤 records 构 anchor，`pipeline.rs:247-308`）、`a885169` 级联删除 rebuild 桩（description/file_path 从不按剩余来源过滤；改为经 `llm_cache_list`→缓存抽取行→`parse_extraction_result` 重建，无记账的旧行保守保留原 description 并 WARN，`purge.rs:329-505`；两处钉桩旧断言按真实语义改写）。CI run 38126377936 fmt+clippy+test 全绿；jev 复评（原 13 条 + 两缺陷闭环共 15 条）**15 verified / 0 contradicted / 0 unsupported**（最低 conf 0.83，PR #2 评论留档）。
- journal 裁决：不引入 journal 表。删除级联走幂等序（chunk 行先删、anchor 行最后删作完成标记，anchor 在场=未完成），中断重跑与一次成功逐字节收敛（`tests/crash_recovery.rs:1039`）；裁决注释在测试头。
- 未完成（登记待办）：
  1. rebuild 片段归属依赖 chunk 行 `llm_cache_list`（新写入均带记账）；无记账旧行保守保留原 description，LLM 重抽归属为后置扩展（`purge.rs:20-26`）。
  2. FAC6 conversation 维已由 R3.T1 在 memory 层落地（PR #4）；RAG pipeline `QueryParam` 过滤与时间过滤已落地（`388b219`，5 测试含缓存键纳键断言，§5 FAC6 行）；B 系列未移植校验项（§7）。
  3. ~~doc01 遗留：turso 崩溃点注入~~（已收口：`tests/turso_crash.rs` 3 用例，`388b219`）。
- 下一动作：R3.T1+T3 已落地（PR #4 squash `44c160b`，jev 11 verified/0 contradicted，canon 全绿）；主线进 R3.T2 向量臂（lancedb scope 过滤 + cache key），图臂按 roadmap 用 turso 邻接表，非 kuzu。
- 2026-10-11 jev 审计方法：条目→代码/测试坐标证据→`jev_verify` 三批 47 条（R1 15 / R2 18 / FAC-TAC 14），contradicted 逐条拆细复核；修复后按「断言 → 当前坐标证据」15 条复评全 verified；全文判定记录见会话产物与 PR #2 评论，关键坐标已内联本文件。
