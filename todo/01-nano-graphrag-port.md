# R1 复刻 nano-graphrag（单 crate 基础闭环）

**状态**：待开工（单 crate 骨架已建：`cargo check` 绿；core 类型/trait/RRF 已就位）。
**目标**：在 Rust 单 crate 中复刻 nano-graphrag 全流程——切块→抽取→合并→建图→社区检测→报告→三种查询；三库替代其 json/nano-vectordb/networkx；LLM 经可替换客户端。
**复刻基准**：`todo/refs/nano-graphrag` @ `acb35c0`（MIT；核心约 2,800 行 + prompt.py 520 行）。codegraph 索引已建（865 节点/1,670 边），本文件所有坐标来自该索引 + 源码核对。
**前置**：无（骨架已可用）。
**下一阶段**：[R2 LightRAG 增强](02-lightrag-upgrade.md)。

## 0. 复刻口径（等价怎么判定）

1. **功能行为等价、数据流等价、查询语义等价**；不逐行翻译 Python，忽略其调试/打印脚手架。
2. 判定手段：
   - **确定性函数**（切块/合并/截断/ID/CSV 组装/JSON 容错解析/社区 schema）：本地可跑参考实现（venv：tiktoken + networkx + graspologic），产出 golden 对照，Rust 输出逐条相等。
   - **LLM 语义部分**：同一 mock 响应喂双端，断言产出的中间结构（`maybe_nodes`/`maybe_edges`/报告 JSON/查询上下文文本）相等。
   - **端到端**：全离线（mock LLM + 本地嵌入），快照断言。
3. 允许偏差必须进 §8 偏差表（例：Leiden 实现差异、嵌入模型差异），不得静默漂移。

## 1. 映射表（源坐标 → 落点）

| nano-graphrag（file:line @ acb35c0） | 作用 | 我们的落点 |
|---|---|---|
| `graphrag.py::GraphRAG` (52-229)：`ainsert` (279-347)、`aquery` (236-275) | 门面装配 + 读写入口 | `src/lib.rs`（`Mnemo` 门面）+ `src/pipeline.rs` |
| `_op.py::chunking_by_token_size` (31-58)、`get_chunks` (94-108) | 切块 + chunk 哈希 | `src/graph/chunk.rs` |
| `_op.py::extract_entities` (282-414) | 抽取主流程（gleaning→解析→合并→实体向量化） | `src/graph/extract.rs` |
| `_op.py::_handle_single_entity_extraction` (138-156)、`_handle_single_relationship_extraction` (159-179) | 记录解析与字段校验 | `src/graph/extract.rs` |
| `_op.py::_merge_nodes_then_upsert` (182-227)、`_merge_edges_then_upsert` (230-279) | 实体/边合并 | `src/graph/merge.rs` |
| `_op.py::_handle_entity_relation_summary` (111-135) | 超限摘要（cheap model） | `src/graph/merge.rs` |
| `_op.py::_pack_single_community_by_sub_communities` (417-461)、`_pack_single_community_describe` (464-600) | 社区上下文装配（预算分配/截断） | `src/graph/report.rs` |
| `_op.py::generate_community_report` (625-697)、`_community_report_json_to_str` (603-622) | 分层报告生成 + JSON→Markdown | `src/graph/report.rs` |
| `_op.py::_find_most_related_community_from_entities` (700-745)、`_find_most_related_text_unit_from_entities` (748-804)、`_find_most_related_edges_from_entities` (807-841)、`_build_local_query_context` (844-932)、`local_query` (935-967) | local 检索链路 | `src/query/local.rs` |
| `_op.py::_map_global_communities` (970-1014)、`global_query` (1017-1104) | global map-reduce | `src/query/global.rs` |
| `_op.py::naive_query` (1107-1140) | naive 纯向量 | `src/query/naive.rs` |
| `_utils.py::compute_mdhash_id` (186-187)、`compute_args_hash` (216-217)、`truncate_list_by_token_size` (169-183)、`clean_str` (241-249)、`split_string_by_multi_markers` (219-224)、`convert_response_to_json` (105-118)、`list_of_list_to_csv` (234-239)、`TokenizerWrapper` (123-165)、`limit_async_func_call` (276-295) | 工具函数 | `src/core/text.rs`、`src/core/concurrency.rs` |
| `_llm.py::openai_complete_if_cache` (38-64)、`gpt_4o_complete`/`gpt_4o_mini_complete` (~126-153)、`openai_embedding` (224-235) | LLM/嵌入客户端 + 缓存 | `src/llm/{client,openai,mock}.rs` |
| `base.py::QueryParam` (10-29)、`BaseKVStorage` (93-113)、`BaseVectorStorage` (78-89)、`BaseGraphStorage` (117-186) | 接口与默认查询参数 | `src/core/traits.rs`（已建，差集补 T1） |
| `_storage/kv_json.py` (46)、`vdb_nanovectordb.py` (68)、`gdb_networkx.py` (268) | 三类存储参考实现 | `src/store/{turso,turso_graph,lancedb}.rs` |
| `prompt.py`：`entity_extraction`/`entiti_continue_extraction`/`entiti_if_loop_extraction`/`summarize_entity_descriptions`/`community_report`/`local_rag_response`/`global_map_rag_points`/`global_reduce_rag_response`/`naive_rag_response`/`fail_response`/`default_text_separator` | 提示词（英文，逐字保留） | `src/graph/prompts.rs`（常量） |

## 2. 常量与默认值（复刻契约；改动=偏差需登记）

| 项 | 值 | 源 |
|---|---|---|
| chunk_token_size / overlap | **1200 / 100**（dataclass 生效值；函数缺省 1024/128 被覆盖） | `graphrag.py:76-79`、`_op.py:36-37` |
| entity_extract_max_gleaning | **1** | `graphrag.py:83` |
| entity_summary_to_max_tokens | **500**（描述 token < 500 不摘要） | `graphrag.py:84`、`_op.py:120` |
| 分隔符 | `GRAPH_FIELD_SEP="<SEP>"`、元组 `<\|>`、记录 `##`、完成 `<\|COMPLETE\|>` | `prompt.py:6,325-327` |
| 实体类型 | `["organization","person","geo","event"]` | `prompt.py:324` |
| 社区 | `max_graph_cluster_size=10`、`random_seed=0xDEADBEEF`、graspologic `hierarchical_leiden`、先取稳定最大连通分量 | `graphrag.py:88-89`、`gdb_networkx.py:230-252,23-52` |
| 向量 | `query_better_than_threshold=0.2`；参考嵌入 `text-embedding-3-small`(1536)；batch 32 | `vdb_nanovectordb.py:13,28`、`_llm.py:223-235`、`graphrag.py:129` |
| 并发 | best/cheap/embedding 各 **16** 并发（`limit_async_func_call` 假信号量） | `graphrag.py:117-131`、`_utils.py:276-295` |
| QueryParam 默认 | mode=global、level=2、top_k=20；naive 12000；local 4000/4800/3200；global min_rating=0/max_consider=512/max_token=16384 | `base.py:10-29` |
| 缓存 | 键=`md5(str((model, messages)))`，`enable_llm_cache=True` | `_llm.py:52`、`_utils.py:217`、`graphrag.py:135` |
| 提交 | 每次 insert **全量 drop 社区报告再重算**（原版 TODO 注明不支持增量） | `graphrag.py:329-330` |
| tokenizer | tiktoken `encoding_for_model("gpt-4o")` → Rust 用 **`tiktoken-rs` 0.12.1**（cl100k/o200k 对齐） | `_utils.py:130-141` |

## 3. 三库落位

| 参考存储 | 数据 | 我们 |
|---|---|---|
| `kv_*`（JSON 文件） | full_docs / text_chunks / llm_response_cache / community_reports / doc_status | turso 表（KV 形态） |
| `vdb_entities/vdb_chunks`（nano-vectordb） | 实体向量（content=name+description，meta entity_name）、chunk 向量（naive 开时） | lancedb |
| `graph_chunk_entity_relation.graphml`（networkx） | 节点：entity_type/description/source_id/clusters(JSON)；边：weight/description/source_id/order | turso 邻接表 |

## 4. 任务表（每条 = 一个可提交闭环；判据=验收）

| ID | 目标（产出） | 源坐标 | 落点 |
|---|---|---|---|
| R1.T1 | 依赖入册 + 类型/trait 差集补齐（tokio/clap/reqwest/sha2/tiktoken-rs/fastembed/leiden-rs/petgraph/turso/kuzu/lancedb/tracing/toml） | `base.py` 四接口清单 | `Cargo.toml`、`src/core/traits.rs` |
| R1.T2 | 切块器：token 窗口切分 + md5 chunk id（`chunk-` 前缀）+ doc 去重 | `_op.py:31-58,94-108`、`_utils.py:186` | `src/graph/chunk.rs` |
| R1.T3 | LLM/嵌入客户端：OpenAI-compatible 生成（重试 5 次退避）、嵌入（本地 fastembed + mock）、参数哈希缓存 | `_llm.py:38-64,224-235` | `src/llm/` |
| R1.T4 | 抽取管线：prompt 装配→调用→gleaning 循环→记录解析（正则/属性数/浮点权重）→maybe_nodes/edges | `_op.py:282-414,138-179` | `src/graph/extract.rs` |
| R1.T5 | 合并：实体（类型众数/描述去重排序/来源拼接/500 触发摘要）+ 边（weight 求和/order 取 min/端点补节点）+ 实体向量化 | `_op.py:182-279,111-135` | `src/graph/merge.rs` |
| R1.T6 | 图存储：`GraphStore` 全方法（含 batch 与 degree）+ clusters 属性读写 + 稳定最大连通分量。实现：`src/store/turso_graph.rs`（turso 邻接表）；**kuzu 偏离原因见 §kuzu 偏离** | `base.py:117-186`、`gdb_networkx.py:23-52` | `src/store/turso_graph.rs` |
| R1.T7 | 社区检测：层次 Leiden（`leiden-rs`）+ 社区 schema（level/occurrence/sub_communities/nodes/edges/chunk_ids） | `gdb_networkx.py:165-252`、`_storage` schema | `src/graph/community.rs` |
| R1.T8 | 社区报告：按 level 自底向上、预算分配（模板开销→子社区→度数排序→节点/边比例截断）、JSON→Markdown | `_op.py:417-697` | `src/graph/report.rs` |
| R1.T9 | 查询三模式：local（vdb→社区/文本/关系三路）/ global（level 过滤→occurrence→rating→分组 map→points→reduce）/ naive | `_op.py:700-1140` | `src/query/{local,global,naive}.rs` |
| R1.T10 | 持久化重载：新进程全量恢复（三库）+ 同查询一致 | `graphrag.py:__post_init__` 恢复逻辑 | `src/store/*` + `src/pipeline.rs` |
| R1.T11 | 并发与提交：三路信号量（16/16/16）可配可观测；index_start/index_done 映射为「turso 先提交→派生库 repair」 | `_utils.py:276-295`、`graphrag.py:349-380` | `src/core/concurrency.rs`、`src/pipeline.rs` |
| R1.T12 | golden fixtures + e2e：参考实现生成 golden（切块/抽取/哈希/合并），全离线 e2e 快照 + 耐久全链 | 全部 | `tests/`、`fixtures/` |

### 逐条判据

- **T2**：Python 侧对 fixture 文本跑 `get_chunks` 输出 golden（content/tokens/chunk_order_index/full_doc_id/md5 id），Rust 逐字段相等；重插同文本零新增。
- **T3**：同 args 二次调用命中缓存（mock 计数=1）；哈希输入构造与 Python 同构（同 model+messages → 同 hex）；网络错误重试 5 次。
- **T4**：固定 mock（含 gleaning 续抽与 `if_loop` 为 no 的短路分支）下产出的 maybe_nodes/maybe_edges 与 Python 相等；`<4`/`<5` 属性拒收、`"entity"`/`"relationship"` 类型判定、`( )` 包裹与反引号清洗。
- **T5**：合并幂等（同输入二次合并图不变）；描述 set 排序拼接、`source_id` 集合、entity_type 计数众数、边 weight 求和、order 取 min、孤立端点自动补 `entity_type="UNKNOWN"` 节点；>500 token 走摘要（mock）。
- **T6**：契约测试全绿（存在性/批量查询/度数/上下位）；`clusters` JSON 往返；重启后图完整。
- **T7**：固定图 fixture（含孤立点/多连通分量）上：层次 level 结构、全部连通节点恰被覆盖一次、同 seed 重跑一致；与 graspologic 的逐节点差异登记进偏差表。
- **T8**：`_pack_single_community_describe` 的 CSV 上下文（Reports/Entities/Relationships 三段）与 Python 输出逐字符相等（同 fixture 图 + 同预算参数）；报告 prompt 组装与 `_community_report_json_to_str` 输出相等。
- **T9**：三模式在 `only_need_context=true` 下返回的上下文文本与 Python 参考相等（同 fixture 库 + 同 mock 嵌入的确定性排序）；global 的 map/reduce 两段分别有断言。
- **T10**：进程 A insert → 进程 B 启动查询，结果与 A 内存态一致；turso 崩溃点注入（提交前/后）不产生半状态。
- **T11**：并发上限可观测（同时挂起数 ≤16）；repair 队列在派生库失败时记录并在下轮补齐。
- **T12**：e2e：小语料 insert→community→3 模式 query 快照；`cargo test` 全离线可过（无网络、无真实 LLM）。耐久版见 `tests/durable_e2e.rs`。

## 5. 三库提交协议（本阶段落地）

三库无共享事务。不变量：**turso 是唯一真值；kuzu/lancedb 是派生索引，任何时刻可重建**。

1. **提交点在 turso 的文档/chunk 行**：`full_docs`/`text_chunks` 在整条插入链**最后**落库（参考实现同序，graphrag.py:342-346）。写在这里之前崩溃 → 该文档视为「未入库」，重跑 insert 从头执行（LLM 缓存让重跑廉价）。
2. **派生库写通过即幂等**：图节点/边与实体向量在过程中 upsert（同键覆盖），重跑不产生重复；`community_reports` 每次插入前 `drop_all`（参考语义）。
3. **重建路径**：`rebuild(scope)` 从 turso 全量重建 lancedb；契约测试：删派生库→重建→检索一致。
4. **错误分类**：`StoreError::{Backend, ScopeViolation, StaleRevision, NotFound}`；「臂失败」与「空结果」可区分。

## 6. 功能验收（运行时可见）

- [ ] R1.FAC1 小语料全链跑通：insert→社区→local/global/naive 三模式各出结果。
- [ ] R1.FAC2 同一文本重复 insert 不产生重复 chunk/实体，图与向量表行数不变。
- [ ] R1.FAC3 图数据可从 turso_graph 读出（节点属性/边权重/来源），社区 schema 层次可见。
- [ ] R1.FAC4 重复查询命中 LLM 缓存（mock/真实调用计数不变）。
- [ ] R1.FAC5 新进程重开后可继续查询，结果与关库前一致。
- [ ] R1.FAC6 `only_need_context` 模式输出与参考实现一致（人工抽查 3 例）。

## 7. 测试验收（自动化）

- [ ] R1.TAC1 切块 golden 对照（含空文本/单 token/超长/中文混合边界）。
- [x] R1.TAC2 存储契约：turso / lancedb / turso_graph 三后端（写读/批量/scope/错误分类/重开恢复）——`tests/{turso_kv,lancedb_vector,turso_graph}.rs`，CI 全绿。
- [ ] R1.TAC3 抽取+合并：mock 全分支（含 JSON 容错解析的三级降级）。
- [ ] R1.TAC4 社区+报告：固定图 fixture 的 schema 与上下文文本快照。
- [ ] R1.TAC5 查询三模式：`only_need_context` 上下文快照 + fail_response 路径。
- [x] R1.TAC6 提交协议：repair 队列（`tests/repair_queue.rs`：派生失败→入队→flush 逐条重放→失败留队）+ 重建等价（`tests/durable_e2e.rs`：删 lancedb 派生库→从 text_chunks 重建→检索一致）。未做：turso 提交前/后的崩溃点注入（需故障注入 harness，R1 未排）。
- [x] R1.TAC7 e2e 离线全链（mock LLM，`cargo test` 无网络）：`tests/pipeline_e2e.rs`（内存后端）+ `tests/durable_e2e.rs`（耐久三后端、跨"进程"重启、派生库重建）。

## 8. 偏差记录（开工时登记，逐条给出批准人）

| 偏差 | 参考 | 我们 | 理由/影响 |
|---|---|---|---|
| Leiden 实现 | graspologic `hierarchical_leiden` | `leiden-rs` | Rust 生态无同款；分区可能不同，schema/流程保持等价（T7 记录差异） |
| 嵌入模型 | OpenAI `text-embedding-3-small` 1536d | fastembed 本地（bge-small 系） | 离线要求；阈值 0.2 为 OpenAI 标定，需在 R6 重标 |
| 并发原语 | `limit_async_func_call` 自旋假信号量 | tokio `Semaphore` | 语义等价（上限控制），实现不同 |
| 图遍历/存储 | networkx 内存图 | turso 邻接表（SQL 度数/邻接） | 语义等价（批量/度数/邻接顺序 API 对齐）；R2 需要多跳/PageRank 时可换图引擎 |
| 抽取内嵌 entity_vdb upsert | `extract_entities` 内做 `entity_vdb.upsert` | pipeline 在 merge 循环后统一 upsert | 数据与顺序等价（仍在 clustering 前）；派生写统一走 repair 队列包装 |
| merge 并发 | `asyncio.gather` 并发 merge 节点/边 | 顺序 merge | merge 之间有图状态依赖，顺序更可复现；抽取本身仍并发（`Limiter`） |

## 8.6 端口审计（2026-10-10，codegraph + 逐函数对照）

对照 `todo/refs/nano-graphrag/nano_graphrag`（`_op.py` 21 函数、`_utils.py` 16、`_llm.py` 9、`prompt.py` 11 模板、`base.py` 3 存储接口）逐项核对：

| 面 | 结论 |
|---|---|
| 提示词 | 11 个模板 10 个逐字符相同（`claim_extraction` 是参考仓死代码——prompt.py 定义但 `grep` 全仓零调用，不移植） |
| 查询参数 | `QueryParam` 12 个字段全部被消费（`level/top_k/response_type/*_max_token_*` 等） |
| facade 配置 | `chunk_*/extract/merge/community/report` 全对齐；`enable_naive_rag` 早有 |
| **缺口 1（已修）** | `enable_local` 未移植：参考仓用它同时 gate `entities_vdb` 构建与 local 查询；已补（不发实体向量 + local 报错） |
| **缺口 2（已修）** | `embedding_batch_num` 是死配置：参考仓按 32 分批 embed，我的 store 一次性 embed；已在 pipeline upsert 处分批 |
| 存储 | 3 接口 → turso KV / turso 图 / lancedb，均有契约测试 |
| 未移植 | claim_extraction（参考死代码）、bedrock/azure provider（不在 R1 范围）、huggingface tokenizer（tiktoken 口径） |

## 8.5 kuzu 偏离（2026-10-10）

**结论**：R1 图存储用 `turso` 邻接表实现，不引入 kuzu。这不是"待评估"，而是被链接期证据劝退的明确决定。

**证据**（GitHub Actions run `hathawayANdRX105/mnemo2u`）：
1. kuzu 0.11.3 静态链把它自带的 vendored simsimd 以 `+whole-archive` 无条件链入；turso_core 又拉同名的 `simsimd` crate → `rust-lld: error: duplicate symbol: simsimd_*`（run 37977636547 kuzu job）。
2. 加 `-Wl,--allow-multiple-definition` 放过重复符号后，真实问题暴露：`undefined symbol: kuzu_rs$cxxbridge1$*`（cxx bridge 的 C++ shim 未链入）→ 静态链在此依赖组合下结构性不可用（run 37982925420）。
3. 上游 kuzu 仓库已归档，无解。

**为什么 turso 邻接表够用**：R1 复刻 nano-graphrag，它的图面就是 `GraphStore` 的那套操作（has/get/upsert/degree/邻接/snapshot），社区检测走 `snapshot()` + leiden-rs，本来就不需要图引擎。turso 邻接表语义与 `MemoryGraph` 完全一致（甚至允许悬空边，比 kuzu 更贴近）。

**回到 kuzu 的触发条件**：R2（LightRAG）需要多跳遍历/PageRank，且届时先验证 kuzu 的 shared 链接模式（`KUZU_SHARED=1`）或升级版能避开 simsimd 冲突。替换成本收敛在 `src/store/turso_graph.rs` 一个文件。

**已落地**：`src/store/turso_graph.rs` + `tests/turso_graph.rs`（属性合并/边/度/邻接顺序/snapshot/scope 隔离/重开恢复），Cargo.toml 移除 kuzu 依赖与 feature，CI 移除 kuzu job（省 30 分钟/轮）。

## 9. 接续记录

- 已完成：
  - 存储后端三件齐：`turso`（KV 真值）、`turso_graph`（图邻接表）、`lancedb`（向量）。每个后端一份契约测试：`tests/{turso_kv,lancedb_vector,turso_graph}.rs`。
  - T1 依赖入册（`turso`/`lancedb`/`leiden-rs`/`tiktoken-rs`/`md-5`/`regex`/`tokio`/`reqwest`/`petgraph` 等；`kuzu` 因上游 vendored C++ 需 `CXXFLAGS=-include cstdint`，暂以 `kuzu-backend` feature 门控）。
  - T2 切块器 + golden 对照（`tools/golden/gen_chunks.py`，11 chunk 逐字段相等）、T3 LLM/嵌入客户端 + 参数哈希缓存（与 Python `md5(str((model, messages)))` 同值，golden 4 例）、T4 抽取管线（gleaning + 解析，golden 5 例）、T5 合并（节点/边 + 摘要三档，golden 4 例）、T7 社区检测（leiden-rs 层次社区 + schema，含确定性/覆盖测试）、T8 社区报告（预算分配 + JSON→Markdown + 并发分相）、T9 查询三模式（local/global/naive，含 `only_need_context`）、T10 管线（`src/pipeline.rs`，含提交点语义）、内存后端（KV/向量/图）。
  - 测试面：`tests/{chunk_parity,hash_parity,extraction_parity,merge_parity,extraction_mock,llm_cache,community_reports,pipeline_e2e,turso_kv}.rs`。
- 未完成：T6 的 kuzu 适配器（GraphStore）、lancedb 向量后端、T11 repair 队列、T12 真实语料烟测。
- 验证方式：测试只在 GitHub Actions 跑（仓库 `hathawayANdRX105/mnemo2u`，`.github/workflows/ci.yml`：fmt + clippy `-D warnings` + `cargo test --all-targets`）；本机不跑测试.
- 下一动作：进入 R2（LightRAG 增强）；R1 收尾前跑一次快照回归（全量 `cargo test`）。
