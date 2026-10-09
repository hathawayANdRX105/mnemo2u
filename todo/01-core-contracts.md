# R1 契约层（core 收口与跨库提交协议）

**状态**：骨架已建（types/traits/RRF，`cargo check` 绿）；本阶段文档待评审后开工。
**目标**：把三条不变量变成可执行契约——派生索引可重建、臂失败可区分、tombstone 写路径拦截；定义三库提交协议。
**前置**：无。
**下一阶段**：[02-write-path](02-write-path.md)。

## 范围与非目标

- 做：类型收口（`Fact`/`Edge`/`ScopeKey`/`TrustState`）、trait 边界（`GraphStore`/`VectorStore`/`FactStore`/`LexicalStore`/`Embedder`）、跨库提交协议、错误分类、契约测试。
- 不做：真 backend 实现（空壳不得注册为可用）、LLM/Jev 接入、检索逻辑。

## 数据契约

### 提交协议（三库无共享事务）

turso 是唯一真值，kuzu/lancedb 是派生索引。写入顺序固定：

1. **turso 提交**：facts 行（含 `revision`）+ 审计 + `organized_through` 水位，**同一事务**。成功即事实成立。
2. **派生库写**：kuzu 边 + lancedb 向量。任一失败：不阻塞事实成立，进入 **repair 队列**（记录 id 与失败库），下轮后台修复。
3. **重建路径**：任何时刻 `rebuild(scope)` 能从 turso 全量重建 kuzu/lancedb。契约测试必须证明「删掉派生库 → 重建 → 检索结果一致（除召回噪声）」。

### 错误分类（臂失败 vs 空结果）

`StoreError` 固定枚举：`Backend`（不可达/损坏）、`ScopeViolation`、`StaleRevision`、`NotFound`（合法空）。
检索聚合层规则：**任一臂返回 `Backend` → 结果带 `degraded: [臂名]` 标记**；`NotFound` 不标记。测试必须覆盖「Turso 挂/向量空」两种可区分输出。

### tombstone 契约

`tombstones{scope_key, normalized_value, rejected_at}`。写路径检查点唯一（网关内），读路径永不查询 tombstone；命中 tombstone 的候选直接拒绝并记审计。

## 任务

- R1.T1 类型收口：`Fact` 增 `embedding_model`（cache key 用）；`Edge` 增 `timestamp`/`conversation_id`（预留给时间臂）；所有类型补 serde round-trip 测试。
- R1.T2 提交协议：定义 `CommitPlan`（turso ops / derived ops / repair 记录），实现 stub 编排器 + 契约测试（注入派生库失败，断言事实仍在 + repair 有记录）。
- R1.T3 错误分类：`StoreError` 落码 + 聚合规则测试。
- R1.T4 `rrf_fuse` 收口：并列 tie 稳定排序（按 id 字典序）、空输入、单臂直通；已实现，补测。
- R1.T5 空壳纪律：`mnemo-store/index/query` 三个 crate 保持 `NOT YET IMPLEMENTED` 文档块，不导出假实现、不注册任何 backend 为可用。

## 验收

- [ ] R1.AC1 `cargo check --workspace` 全绿；`cargo test -p mnemo-core` 通过（round-trip / rrf / 错误分类）。
- [ ] R1.AC2 提交协议测试：派生库注入失败后 turso 事实完整、repair 队列有记录、`rebuild` 可恢复。
- [ ] R1.AC3 臂失败与空结果在聚合 API 层面可区分（单元级）。
- [ ] R1.AC4 无假实现：`grep -r "todo!\|unimplemented!" crates/` 为空；三个空壳 crate 无公开实现导出。

## 验证与烟测

本地：`cargo fmt --check`、`cargo check --workspace`、`cargo test -p mnemo-core`。
CI（后续接入）：AC1–AC4 全跑。

## 接续记录

- 已完成：类型/trait/RRF 初版。
- 未完成：T1 增量字段、T2 协议实现、T3/T4 测试、T5 核查。
- 下一动作：评审通过后按 T1→T5 顺序开工。
