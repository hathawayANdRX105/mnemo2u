# R1 复刻 nano-graphrag（单 crate 基础闭环）

**状态**：待开工（单 crate 骨架已建：`cargo check` 绿；core 类型/trait/RRF 已就位）。
**目标**：在 Rust 单 crate 中**复刻 nano-graphrag 的完整流程**——切块→抽取→合并→建图→社区检测→报告→三种查询；用三库替代其 json/nano-vectordb/networkx；LLM 经可替换客户端。
**复刻基准**：`todo/refs/nano-graphrag` @ `acb35c0`（MIT，核心约 1100 行 + prompts）。
**前置**：无。
**下一阶段**：[02-lightrag-upgrade](02-lightrag-upgrade.md)。

## 复刻定义

- **同功能行为**：同样的数据流、同样的查询语义、同样的持久化语义（重开进程可继续）；不逐行翻译 Python。
- **替换点**（有意偏离，写在明处）：

| nano-graphrag 组件 | mnemo2u 实现 | 说明 |
|---|---|---|
| `_splitter.py`（token-size 切块） | `src/graph/chunk.rs`（jieba 分词近似计数） | 中文用 jieba-rs；英文按空白/标点近似 |
| `_llm.py`（OpenAI/Azure/Bedrock） | `src/llm/`（`LlmClient` trait + OpenAI-compatible HTTP + mock） | CI 用 mock，零网络 |
| `_op.py`（extract/merge/community/query） | `src/graph/ops/` 拆文件 | 大函数拆小模块 |
| `prompt.py` | `src/graph/prompts/` 常量 | 保持与参考一致的英文提示词 |
| `_storage/kv_json.py` | `src/store/turso.rs`（KV 表） | 真值层 |
| `_storage/vdb_nanovectordb.py` / `vdb_hnswlib.py` | `src/store/lancedb.rs` | ANN |
| `_storage/gdb_networkx.py` / `gdb_neo4j.py` | `src/store/kuzu.rs` | 图数据库（嵌入式） |
| `base.py` | `src/core/traits.rs` | 已建 |
| `graphrag.py`（facade） | `src/lib.rs` 的门面类型 | insert/query 两个入口 |

## 功能清单（本阶段实现什么）

- F1 `insert(text)` / `ainsert`：切块（token-size + overlap）→ sha256 去重 → 并发 LLM 抽取 → 合并 → 写三库。
- F2 实体/关系抽取：JSON 容错解析（参考 `_utils.py` 的 body 定位）；含描述、关系权重（重复次数）。
- F3 合并（merge）：同名实体/关系聚合描述、权重、来源 chunk 引用；`merge_entities`/`merge_relationships` 语义对齐参考。
- F4 图构建：节点=实体（kuzu node 属性存描述），边=关系（属性存描述/权重/来源）。
- F5 社区检测：**Leiden 层次社区**（`leiden-rs`）——对齐参考用 graspologic 的效果；叶子→上级递归。
- F6 社区报告：自底向上 LLM 生成（对齐参考的 report prompt），报告文本 + 向量入库。
- F7 查询三模式：
  - `local`：实体向量召回 → 邻域边 + 源 chunk → 组装答案；
  - `global`：全量社区报告 → map-reduce（含 helpfulness 过滤）；
  - `naive`：纯 chunk 向量 RAG。
- F8 LLM 响应缓存：`hash(model, prompt)` 键，turso KV；重放查询零新调用。
- F9 异步：`ainsert`/`aquery`（tokio），并发受配置上限约束。
- F10 持久化重载：新进程凭 turso 库继续（对应参考的 working_dir 语义）。

## 三库落位（本阶段）

| 数据 | 落点 |
|---|---|
| full_docs / text_chunks / llm_cache / community_reports / doc_status | turso（KV 表 + 状态列） |
| 实体、关系、社区 | kuzu（节点/边/社区分组） |
| chunk 向量、实体向量、报告向量 | lancedb（三张表，`hash(scope,model,text)` 键） |

> 本阶段沿用参考的 KV 形态落地；R2 再把 facts/edges 升级为正式模型（双时态/trust_state/scope 主键），届时提供迁移。

## 三库提交协议（本阶段落地）

三库无共享事务。不变量：**turso 是唯一真值；kuzu/lancedb 是派生索引，任何时刻可重建**。

1. **turso 先提交**：文档/chunk/缓存/状态（含幂等键）同事务落库。成功即事实成立。
2. **派生库后写**：kuzu 节点/边 + lancedb 向量。任一失败不阻塞事实成立，记入 **repair 队列**（id + 失败库），下轮重试补齐。
3. **重建路径**：`rebuild(scope)` 从 turso 全量重建 kuzu/lancedb。契约测试：删派生库 → 重建 → 检索结果一致。
4. **错误分类**：`StoreError::{Backend, ScopeViolation, StaleRevision, NotFound}`；聚合层「臂失败」（Backend）与「空结果」（NotFound）必须可区分。

## 任务

- R1.T1 crate 骨架：Cargo.toml 依赖入册（tokio/clap/reqwest/sha2/jieba-rs/fastembed/leiden-rs/petgraph/turso/kuzu/lancedb/tracing）+ 配置结构。
- R1.T2 `LlmClient` trait + mock + OpenAI-compatible HTTP（超时/重试固定写）。
- R1.T3 切块器（`src/graph/chunk.rs`）+ 单测（中英混合、边界、overlap）。
- R1.T4 抽取器（`extract.rs` + prompts）：JSON 容错、失败重试语义。
- R1.T5 合并器（`merge.rs`）。
- R1.T6 turso backend 落地（KV + 状态；契约测试）。
- R1.T7 lancedb backend 落地（三表；契约测试）。
- R1.T8 kuzu backend 落地（GraphStore trait；契约测试）。
- R1.T9 社区检测（leiden-rs）+ 报告生成。
- R1.T10 查询三模式（local/global/naive）。
- R1.T11 e2e（mock LLM，夹具书文本）：insert→community→query 快照断言。

## 功能验收（运行时可见）

- [ ] R1.FAC1 夹具文本跑通 `insert → local/global/naive 三种 query`，三者各自有非空、可区分的结果。
- [ ] R1.FAC2 同一文本插入两次：chunk 零重复（sha256 幂等），第二次零 LLM 调用（缓存计数）。
- [ ] R1.FAC3 kuzu 中可读出实体/边/权重/社区分组；lancedb 三表可 ANN 查询。
- [ ] R1.FAC4 新进程重开同一库：三种查询结果与关闭前一致（持久化语义）。
- [ ] R1.FAC5 删除一条文档（R1 范围：doc_status 标记 + 图/向量标注，不做级联）后，local 查询不再返回其 chunk。

## 测试验收（自动化）

- [ ] R1.TAC1 单测：切块边界 / JSON 容错（缺字段、尾随文本、markdown 围栏）/ merge 聚合（同名不同 chunk 合并，权重相加）。
- [ ] R1.TAC2 backend 契约测试 ×3：写读回环、scope 过滤、错误分类（`Backend`/`ScopeViolation`/`NotFound`）。
- [ ] R1.TAC3 e2e（mock）：insert→merge→community→report→query 全链，产物对快照（切块数、实体数、边数、社区数、报告数）。
- [ ] R1.TAC4 幂等：insert 同文本两次，DB 行数与图规模不变。
- [ ] R1.TAC5 零网络：全套测试不发真实 HTTP（mock 生效断言）；fastembed 用离线缓存。
- [ ] R1.TAC6 无假实现：`grep -rn "todo!\|unimplemented!" src/` 为空；空 backend 不得注册可用。

## 验证与烟测

- 本地：`cargo fmt --check`、`cargo check`、`cargo test`（mock 路径全离线）。
- 烟测（授权环境）：真实 LLM 小样本跑一遍 insert→query，记录实际模型与 token 用量。

## 接续记录

- 已完成：单 crate 骨架（core 类型/trait/RRF），`cargo check` 绿。
- 未完成：T1–T11 全部。
- 下一动作：T1 依赖入册 → T2/T3 并行（LLM 客户端与切块器互不依赖）。
