# R5 反思更新（corroborate / supersede / 失效传播）

**状态**：待实施。
**目标**：让记忆可以「被更正且更正抗重建」——新证据到来时对受影响条目做 corroborate/supersede/失效重查，后台整理不阻塞主循环。
**前置**：[R4](04-jev-integration.md)验收。
**下一阶段**：[06-evaluation](06-evaluation.md)。

## 范围与非目标

- 做：更新操作集、独立来源计数、tombstone 生命周期、来源失效传播、后台整理（单飞 + 有界）。
- 不做：跨 workspace 长期记忆（需另行授权）、自动晋升全局偏好、社区摘要（本库是检索型记忆，不做语料全局问答）。

## 契约

### 更新操作（写网关内扩展，复用 R2 提交协议）

| 操作 | 触发 | 语义 |
|---|---|---|
| corroborate | `jev_compare` = same_fact 且来源独立 | 提升 confidence/来源计数；**按 session/event 去重**，同一会话重复不算独立 |
| supersede | 新证据与旧条目矛盾且新证据可信 | 旧条目闭区间（`valid_to`）、链 `superseded_by`；不删除 |
| reject + tombstone | 用户/宿主显式拒绝 | 值摘要进 tombstone；后续抽取无法回潮 |
| invalidate | 来源事件被改/删（rewind、删除会话） | 级联失效派生条目，重建后不复活 |
| revalidate | stale 条目被新证据重新支持 | stale → verified，保留历史 |

### 后台整理（参考 TRACE 分层树的形态，不抄实现）

- 单飞：每 scope 同时最多一个整理任务；有界队列，超限合并目标版本。
- 完成才推进派生状态；失败保留输入、可重试；晚到结果按 `source/focus/config` 版本校验，过期丢弃。
- 不阻塞写读主路径：读路径允许读到「未整理完成」的旧视图，但带 `organizing: true` 标记。

### 独立来源计数（防自证循环）

`corroboration_count` 只对**不同 session_id 或不同 event_id** 的来源递增（CSM 模式）；重复抽取同一来源不计。

## 任务

- R5.T1 更新操作：网关扩展（corroborate/supersede/reject/invalidate/revalidate）+ 单元与集成测试。
- R5.T2 独立来源：计数键与去重测试（同会话重放不增量、跨会话增量）。
- R5.T3 tombstone 生命周期：写入路径的拦截测试 + 审计；tombstone 不进召回（断言）。
- R5.T4 失效传播：来源删除/rewind 仿真 → 派生条目级联失效 → 重建后不复活。
- R5.T5 后台整理器：单飞、有界、版本校验、失败重试；与主路径隔离测试。
- R5.T6 stale 判定：时效规则（配置化）→ stale 标记；revalidate 拉回。

## 验收

- [ ] R5.AC1 更正抗重建：执行更正 → 全量重建派生索引 → 更正仍生效（atlas：rebuild 会 undo 未入重建输入的更正——测试抓这个）。
- [ ] R5.AC2 独立来源：同会话 50 次重放计数不变；两个独立会话计数 +2。
- [ ] R5.AC3 tombstone 拦截：被拒绝值 100 次抽取尝试零回潮；不进 prompt（含检索侧断言）。
- [ ] R5.AC4 失效传播：删除来源 → 受影响条目全失效；其余条目不受影响（最小爆炸半径）。
- [ ] R5.AC5 后台整理失败/取消/晚到：不覆盖新状态；输入保留可重试。
- [ ] R5.AC6 stale：命中时效规则的条目转 stale 且默认不放行；revalidate 恢复。

## 验证与烟测

夹具：构造「用户更正 → 重启 → 重建」全链。烟测：隔离会话中做一次真实更正（改早前事实），跑重建，再查询验证。

## 接续记录

- 已完成：无。
- 未完成：T1–T6。
- 下一动作：R4 验收后定更新操作的状态迁移表。
