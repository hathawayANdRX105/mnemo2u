# R2 写路径（切片 → 候选 → 选择性 Jev → 治理写网关）

**状态**：待实施。
**目标**：把 harness 的 closed-turn 证据转成记忆条目——零-LLM 候选先行，只有高价值 chunk 才过 Jev（KET-RAG 式），全部经单一写网关原子提交 + 水位推进。
**前置**：[R1](01-core-contracts.md)验收。
**下一阶段**：[03-retrieval](03-retrieval.md)。

## 范围与非目标

- 做：输入适配器（harness closed turn 读取接口）、本地候选生成、价值筛选、Jev 选择性抽取、去重/冲突判定、治理写网关、水位与 repair。
- 不做：检索逻辑（R3）、外发授权流程（R4 细化，本阶段先用「默认关闭」占位）、跨 workspace 汇总。

## 流水线

```
Reader(harness) ──closed turns──▶ 切片器（按 turn/工具组边界）
   → 本地候选：实体/关键词/路径/ID（regex + 分词，零 LLM）
   → 价值评分：信息密度启发式（实体数/新路径数/长度）
   → [score ≥ 阈值] → jev_extract/classify（有界候选）
   → 治理写网关：
        scope 校验 → 近邻去重（embedding 预筛 + jev_compare）
        → 冲突（contradicts→双保留+conflicted）→ tombstone 检查
        → CommitPlan（turso 真值+水位 → 派生库 → repair）
   → organized_through 推进（与 turso 提交同事务）
```

### 关键契约

1. **幂等**：水位 `organized_through` 与 turso 提交同事务；事件级幂等键（`session_id+event_id`）；重跑同一批次零新增。
2. **去重成本控制**：先 embedding 近邻（lancedb top-k）筛小集合，仅对近邻调 `jev_compare`；绝不全量两两比较。
3. **冲突保留**：`contradicts` 不覆盖——新条目 `candidate` + 旧条目保持，互链 `Corrects`/`Supersedes` 边待 R5 处置。
4. **禁止空写**：Jev 不可用/超时 → 该 chunk 停为「未整理」，水位不越过后继续（下次重试）；不允许伪造抽取结果。
5. **价值阈值**：初值待实测决定（R2 开工时用真实会话样本标定），配置化，不硬编码产品常量。

## 任务

- R2.T1 Reader 适配器：定义 `EvidenceReader` trait（closed turns + 事件元数据），harness 侧实现留桩；单测用夹具。
- R2.T2 切片器：按 turn/工具组边界切片，闭合组不可拆；产出 `SourceRef` 列表。
- R2.T3 本地候选：regex/路径/ID/关键词抽取 + 价值评分；单测覆盖中英文混合、代码路径、错误码。
- R2.T4 Jev 选择性抽取：仅高于阈值 chunk 调用；候选上限、超时、失败语义（unresolved≠空）；外发门默认关闭（拒绝即跳过）。
- R2.T5 写网关：去重/冲突/tombstone/CommitPlan 编排；幂等、回滚、repair 记录。
- R2.T6 水位：与 turso 事务同提交；崩溃恢复测试（提交后崩溃、提交前崩溃两种）。

## 验收

- [ ] R2.AC1 同一批 closed turns 重放两次：facts 零重复、水位单调、审计每条写入可追。
- [ ] R2.AC2 低价值 chunk 零 Jev 调用（计数断言）；高价值 chunk 抽取失败不推进水位、不产生空条目。
- [ ] R2.AC3 tombstone：被拒绝值经后续抽取无法重新入库；冲突双保留且带状态。
- [ ] R2.AC4 派生库注入失败：事实完整、repair 有记录、重试可补齐；崩溃恢复后水位与事实一致。
- [ ] R2.AC5 `organized_through` 跨重启单调不减。

## 验证与烟测

单测 + 集成（夹具会话，不接真实 LLM 的部分走确定性路径）。
烟测：用隔离工作区真实会话跑一轮「插入 → 拔库 → 重启 → 继续」，验证幂等与恢复；Jev 部分在有授权环境单独跑，记录实际模型。

## 接续记录

- 已完成：无。
- 未完成：T1–T6。
- 下一动作：R1 验收后定义 `EvidenceReader` 接口与夹具格式。
