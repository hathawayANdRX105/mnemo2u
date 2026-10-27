# R3 读路径（scope 前滤 → 多臂召回 → 融合 → 分层返回）

**状态**：已实施——T1–T7 全部落地（PR #4 squash 合并 `44c160b` 落 T1/T3；PR #5 squash 合并 `5d6c74c` 落 T2/T4/T5/T6/T7，2026-10-12；两轮 CI 全绿、jev 审计 11+14 verified / 0 contradicted、canon 全量 diff ALL PASS）；R4 jev_rerank 接线已随 PR #7 落地（`MemoryRecall.judge` rerank+find 全链）；harness 侧 L1/L2 消费与真实会话烟测待后续阶段。
**目标**：以最小延迟给 harness 返回「带 SourceRef 的候选」——先把 scope 钉死，再多臂召回、融合、可选用 Jev 精排，最后按 L0→L2 分层交付。
**前置**：[R2](02-write-path.md)验收。
**下一阶段**：[04-jev-integration](04-jev-integration.md)。

## 范围与非目标

- 做：scope 硬前滤、三臂（向量/BM25/图邻接）+ 可选时间臂、RRF 融合、截断预算、L0/L1/L2 分层、降级标记。
- 不做：rerank 的 Jev 实现（R4；本阶段接口留好）、生成答案（库不生成，只交付证据）。

## 检索契约

### 臂与顺序（固定）

```
query
 → scope 解析（workspace/session/task 校验，未授权即拒绝——排序前）
 → 臂1 向量（lancedb top-k，scope 过滤在搜索内）
 → 臂2 BM25（turso fts_match/fts_score，scope 条件在查询内）
 → 臂3 图邻接（kuzu 1–2 跳 expansion，仅接受臂1/臂2 的命中为种子）
 → [臂4 时间邻接，可选] 仅当 query 含时间表达时参与（否则不稀释融合）
 → RRF 融合（k=60 起步，配置化；并列按 id 稳定排序）
 → [可选 jev_rerank，接口在 R4 接]
 → exists 判定 → 截断到预算 → 分层返回
```

### 关键契约

1. **scope 在每臂内部生效**（不是融合后过滤）——否则窄权限用户拿到被挤空的最差结果（atlas：Muninn 教训）。否定测试：跨 scope 泄漏必须让测试红。
2. **降级可观测**：`degraded: [臂名]` 随结果返回；臂失败≠空结果（R1 错误分类承接）。
3. **L0/L1/L2**：默认返回 L0（条目摘要 + SourceRef + trust_state）；`L1` 详情、`L2` 原文由 harness 用 `context_read` 按 SourceRef 取（本库不存原文）。
4. **候选上限**：Jev 精排候选 ≤ 250（tool 限额）；融合只对候选集做，不回流全库。
5. **延迟预算**：热路径目标——三臂本地执行 < 50ms（千~十万级条目）；含 jev_rerank 的网络调用单列记录，不计入本地预算。

## 任务

- [x] R3.T1 scope 前滤：三臂公共谓词 + 否定测试（跨 workspace/session/task 泄漏）。**已落地（PR #4）**：`ScopeKey::admits`（`types.rs`）为唯一共享谓词；`MemoryFactStore`（`store/facts.rs`）在存储内前滤；否定测试 `tests/scope_prefilter.rs`（7 例）。
- [x] R3.T2 向量臂：lancedb 集成（真实 backend 首个落地）+ cache key 校验（`hash(scope,model,text)`）。**已落地（PR #5）**：`LanceFactVector`（`store/lance_facts.rs`，scope 谓词 `.only_if` 进 ANN 扫描、prefilter 语义、从不 postfilter；cosine 降序+id 破平）+ `fact_embedding_cache_key`；`tests/vector_arm_scope.rs` 以「ws-b 更同向向量在 ws-a 不可见」同时证明 prefilter。偏差：逐行 merge_insert ~0.5s/op——AC4 万级夹具在测试侧批量直写表数据（优化留待真实写入方接入）。
- [x] R3.T3 词法臂：turso FTS（`fts_match`/`fts_score`，**非 FTS5 语法**）封装为 `LexicalStore`；精确标识符（路径/错误码/ID）召回测试。**已落地（PR #4）**：`TursoLexical`（`store/turso_lexical.rs`，`USING fts` + `experimental_index_method(true)`，单 SQL 内 scope 前滤）；`tests/lexical_recall.rs` 标识符 top-1 召回。偏差记录：`IFNULL` 表达式主键被本地引擎拒绝→Rust 端 `None→""` 归一化；引号翻倍触发 FTS 解析错误→反斜杠转义（模块文档记录原因）。
- [x] R3.T4 图臂：kuzu 邻接扩展（1–2 跳），种子仅取臂1/2 命中；权重含 `rel` 类型加权。**已落地（PR #5，按 roadmap 改用 turso 邻接表，非 kuzu）**：`TursoFactEdges`（行携写入方 scope、每跳 BFS 查询内嵌 scope 谓词、无向、防环）；rel 乘数（Corrects/Supersedes 1.5、DependsOn/AboutTask 1.2、其余 1.0）在编排器 `rel_multiplier`；种子=臂1/2 各前 10（`query/facts.rs`）。
- [x] R3.T5 时间臂（可选）：query 时间表达检测（本地规则），命中才启用；时间邻接权重 = f(时间距离, conversation 邻近)。**已落地（PR #5，保守形态）**：`temporal_centre`（YYYY-MM-DD/今天/昨天/前天/today/yesterday，锚点落日网格）；命中仅对融合结果按 Fact 时间距窗口中心**稳定重排**（不注入新候选，不稀释融合）；conversation 邻近维落在 RAG pipeline 过滤面（`QueryParam.conversation_id`/`time_window`，`388b219`）；memory 臂侧仍按会话水位后补。
- [x] R3.T6 融合与截断：RRF + 预算（字符/token 估计）+ 稳定排序。**已落地（PR #5）**：`rrf_fuse`（k 可配，同分 id 字典序稳定——本轮修掉 HashMap 迭代乱序缺陷）；`per_arm_limit` 钳 Jev 250 上限（契约 4）；融合后 `top_k` 截断。
- [x] R3.T7 分层返回：L0 结构 + SourceRef；L1/L2 接口留给 harness。**已落地（PR #5）**：`L0Candidate`（summary 200 char 边界截断 + SourceRef + trust_state + score + via）；真值层精确 scope 物化、supersede 自动出局；L1/L2 由 harness 按 SourceRef 消费（库不存会话原文，契约 3）。

## 验收

- [x] R3.AC1 精确标识符（路径、错误码、符号名）召回命中率达标（夹具集上对比纯向量臂显著更优）。（词法臂 top-1：`tests/lexical_recall.rs`；对比纯向量臂：`tests/memory_recall.rs::ac1`——MockEmbedder 向量臂漏检/靠后，vector+lexical 融合目标 top-1。）
- [x] R3.AC2 跨 scope 否定测试全红→修后全绿；未授权 scope 在排序前即拒绝。（三臂全部：`scope_prefilter.rs`、`lexical_recall.rs` §2、`vector_arm_scope.rs`、`graph_arm.rs` 各带对照；e2e `memory_recall.rs::ac2` 三臂镜像 ws-b 零泄漏。）
- [x] R3.AC3 单臂故障注入：结果仍返回且带 `degraded`；空臂不标记。（`memory_recall.rs::ac3` 两例：单臂 Err 命名、三臂全灭 exists=false、健康空臂不标记。）
- [x] R3.AC4 延迟：本地三臂 P50/P95 记录（万级条目夹具），纳入回归基线。（`memory_recall.rs::ac4`：10k 行三臂、101 次、实测 **P50=104ms P95=233ms**、断言 P95<500ms。偏差：超「<50ms 热路径目标」2–4 倍——目标值口径，根因=万级 lancedb ANN+FTS+真值扫描，优化登记未完成项。）
- [x] R3.AC5 RRF 消融：任一臂的增量贡献有数据（不需要每臂都赢，但必须可测）。（`memory_recall.rs::ac5`：词法独有 id 去臂即消失；rrf_k 拨盘改变名次。）

## 验证与烟测

夹具：构造含路径/错误码/跨任务/时间表达的场景集。
烟测：真实 harness 会话（隔离）跑「压缩后回查早期错误原文」，验证从 L0 → SourceRef → context_read 全链。

## 接续记录

- 已完成：T1–T7 全部。PR #4（`44c160b`，T1/T3）、PR #5（`5d6c74c`，T2/T4/T5/T6/T7 + `FactVectorStore` 声明面 + `FactEdgeStore` scope 写面 + `rrf_fuse` 确定性修复）。jev 两轮 11+14 verified / 0 contradicted（PR #4/#5 评论留档）；canon pre-commit 全量 diff 两轮 ALL PASS。
- 未完成（登记待办）：~~R4 接 `jev_rerank`~~（已随 PR #7 落地：`MemoryRecall.judge` rerank+find 全链）；harness 侧 L1/L2 消费与真实会话烟测（§验证与烟测，需 C 系列账本接入）；~~T5 的 conversation 邻近维~~（RAG pipeline 过滤面已落，`388b219`；memory 臂侧仍需会话水位）；万级延迟优化（AC4 偏差 104/233ms vs 50ms 目标）；lancedb 逐行 upsert 性能（~0.5s/op）。
- 下一动作：无（R4 精排接线已随 PR #7 落地）；后续为 harness 侧 L1/L2 消费与真实会话烟测（需 C 系列账本接入）。
