# R3 读路径（scope 前滤 → 多臂召回 → 融合 → 分层返回）

**状态**：待实施。
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

- R3.T1 scope 前滤：三臂公共谓词 + 否定测试（跨 workspace/session/task 泄漏）。
- R3.T2 向量臂：lancedb 集成（真实 backend 首个落地）+ cache key 校验（`hash(scope,model,text)`）。
- R3.T3 词法臂：turso FTS（`fts_match`/`fts_score`，**非 FTS5 语法**）封装为 `LexicalStore`；精确标识符（路径/错误码/ID）召回测试。
- R3.T4 图臂：kuzu 邻接扩展（1–2 跳），种子仅取臂1/2 命中；权重含 `rel` 类型加权。
- R3.T5 时间臂（可选）：query 时间表达检测（本地规则），命中才启用；时间邻接权重 = f(时间距离, conversation 邻近)。
- R3.T6 融合与截断：RRF + 预算（字符/token 估计）+ 稳定排序。
- R3.T7 分层返回：L0 结构 + SourceRef；L1/L2 接口留给 harness。

## 验收

- [ ] R3.AC1 精确标识符（路径、错误码、符号名）召回命中率达标（夹具集上对比纯向量臂显著更优）。
- [ ] R3.AC2 跨 scope 否定测试全红→修后全绿；未授权 scope 在排序前即拒绝。
- [ ] R3.AC3 单臂故障注入：结果仍返回且带 `degraded`；空臂不标记。
- [ ] R3.AC4 延迟：本地三臂 P50/P95 记录（万级条目夹具），纳入回归基线。
- [ ] R3.AC5 RRF 消融：任一臂的增量贡献有数据（不需要每臂都赢，但必须可测）。

## 验证与烟测

夹具：构造含路径/错误码/跨任务/时间表达的场景集。
烟测：真实 harness 会话（隔离）跑「压缩后回查早期错误原文」，验证从 L0 → SourceRef → context_read 全链。

## 接续记录

- 已完成：无。
- 未完成：T1–T7。
- 下一动作：R2 验收后先落 lancedb 与 turso 两个真实 backend（kuzu 随后）。
