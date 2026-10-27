# R5 反思更新（corroborate / supersede / 失效传播）— 契约 v2

**状态**：契约 v2（2026-10-16 重审后冻结）；**已实施并全部合并进 main**（四片 squash：#14 `911a6c6` →
#15 `25c6e8a` → #16 `04b5332` → #17 `53ff0b2`）；main 三次 push CI 全绿；分支与 worktree 已清。
**v2 变更**（相对 v1 的删减，全部有理由）：
- **删「更正账本 + 重放」**：派生索引重建本就从真值层重建（`durable_e2e` 已钉），facts 行即更正记录；
  账本 = 第二真值面，删。AC1 从"新机制"降级为"回归测试"。
- **删「后台整理器」整片（原 T5）**：无必需消费者——stale 改读时派生；级联失效由来源删除事件同步触发。
  背景化自身引入的失败模式（晚到/版本/单飞/重试）随之消失。**AC5 一并删除**。
- **删 source→fact 倒排索引**：删除事件时扫 scope 过滤 `source_refs`；一次扫描 ≤ 一次查询成本。
- **删 `corroboration_count` 新字段**：计数从 `source_refs` 的 distinct `(session_id, event_id)` 集合派生。
- **`TrustState::Stale` 变体删除**：读时派生后无生产者（atlas 点名的"无生产者状态"）。`Rejected` 由 reject 给生产者。
- TRACE 的借鉴收敛为"读时 stale 规则"，不是进程。

**目标**：让记忆可以「被更正且更正抗重建」——新证据到来时对受影响条目做 corroborate/supersede/失效重查。
**前置**：[R4](04-jev-integration.md) ✓（PR #7）。写网关（PR #11 `172c15d`）为承载面。

## 更新操作（写网关 `FactWriteGateway` 内，复用 R2 提交协议）

| 操作 | 触发 | 语义 | 落点 |
|---|---|---|---|
| corroborate | `judge compare` = same_fact | 旧行 `source_refs` append（按 `(session_id,event_id)` 去重）；distinct session ≥ 2 且 `Candidate` → 升 `Verified`；同会话重放零增量 | `commit_turns` SameFact 分支（现为直接丢弃） |
| supersede | compare = contradicts | 旧行闭 `valid_to`（= 新行 `created_at`，确定性）+ `superseded_by` + revision+1；不删除 | 已有 supersede，补 `valid_to` |
| reject + tombstone | 宿主显式拒绝（新方法 `reject()`） | 值摘要（归一化文本 md5，前缀 `tomb-`）进 tombstones 表；judge **前**拦截；召回排除 | gateway + tombstone KvStore 表（与 watermarks 同构） |
| invalidate | 来源事件被删（新方法 `invalidate_source()`） | 扫 scope：`source_refs` 含该 `(session,session_id..event_id)` 的行闭 `valid_to`；`DependsOn` 边级联 | gateway |
| revalidate | stale/候选行被新证据 corroborate | 由 corroborate 自然覆盖（append + 升级）；保留历史（revision 不重置） | 同上 |

**读侧统一谓词**（两后端 + 召回共用，单一来源）：

```rust
impl Fact { pub fn is_current(&self) -> bool { self.superseded_by.is_none() && self.valid_to.is_none() } }
```

- memory 后端：`search` retain `is_current()`；turso 后端：SQL 保 `superseded_by IS NULL`，解码后补 `valid_to.is_none()`
  （valid_to 在 body JSON 内，search 本来就逐行解码，零新增成本）。
- `chunk_hit_admits`（RAG chunk 臂）不动——那是 QueryParam 过滤，与 fact 双时态正交。

**stale（读时派生，替代原 T6）**：`RecallOptions.staleness: Option<StaleRule{max_age_secs, min_sessions}>`，
`None`=现状（全放行）。命中 → 不返回（与 is_current 同层执行），**不写状态**。

## 纪律

- 独立来源计数键：`(session_id, event_id)`（SourceRef 现成）；同一会话重复不算独立（CSM 模式）。
- tombstone 永不 TTL（atlas：「A tombstone that expires is not a tombstone」）；key 用归一化摘要（挡值不挡字符串）。
- 阈值常量：`CORROBORATION_THRESHOLD = 2`（cambium/clio 同源）；不做配置面（YAGNI）。

## 任务（4 PR，每片一机制）

| PR | 内容 | 覆盖 |
|---|---|---|
| **1. 更新操作核** | `is_current` 谓词双端落地 + supersede 闭 `valid_to` + corroborate（SameFact 改造）+ 抗重建回归测试 | AC1 + AC2 |
| **2. tombstone** | tombstones 表 + judge 前拦截 + 召回排除 + `reject()` | AC3 |
| **3. invalidate 传播** | `invalidate_source()` 扫 scope 级联 + `DependsOn` 边 | AC4 |
| **4. stale 读时派生** | `RecallOptions.staleness` + revalidate 行为验证 | AC6 |

## 验收

- [ ] R5.AC1 更正抗重建：执行更正 → 重建派生索引 → 更正仍生效。（回归测试，非新机制）
- [ ] R5.AC2 独立来源：同会话 50 次重放计数不变；两个独立会话 → distinct=2 且升 `Verified`。
- [ ] R5.AC3 tombstone 拦截：被拒值 100 次抽取尝试零回潮；不进召回（含检索侧断言）。
- [ ] R5.AC4 失效传播：删除来源 → 受影响条目全失效；其余不受影响（最小爆炸半径）。
- [ ] R5.AC6 stale：命中时效规则条目默认不放行；corroborate 恢复可见。

## 接续记录

- 已完成：契约 v2（本文件；删减项见顶部「v2 变更」）。
- **PR-1（更新操作核）已开**：PR #14，分支 `feat/r5-update-ops`（commit `bda9613`）。交付：
  `Fact::is_current()`（双后端 search 共用；turso 的 `valid_to` 半在解码后查）+ `supersede` 闭
  `valid_to`（= 新行 `created_at`）+ 网关 `SameFact` → corroborate（`(session_id,event_id)` 去重，
  distinct session ≥2 升 `Verified`，revision 不动）+ 邻居过滤改 `is_current`。测试坐标：
  `tests/scope_prefilter.rs::valid_to_closed_rows_leave_search`（memory）、
  `tests/turso_facts.rs::valid_to_closed_rows_leave_search`（durable）、
  `tests/write_gateway.rs::same_session_restatements_do_not_add_independent_sources` /
  `::independent_conversations_promote_corroboration_to_verified`（AC2 两半）。全量 36 target 绿、
  clippy 0、fmt 干净。
- 登记偏差：AC1 的派生索引重建口径待重建接缝（重放半边已由既有
  `superseded_facts_are_not_resurrected_by_replay` + 本片重放断言覆盖）。
- **PR-2（tombstone，AC3）已开**：PR #15（draft，堆叠在 #14 之上），分支 `feat/r5-tombstone`
  （commit `1093e18`）。交付：`reject()`（scope 级值摘要账本；行留档 `Rejected` + revision bump）、
  `is_current` 纳入 `Rejected`、judge **前**批量拦截（`GatewayOutcome.tombstoned`）。
  测试坐标：`tests/write_gateway.rs::rejected_values_never_reenter_and_leave_recall`（100 次变体重抽零写入、
  零 judge 调用、账本 1 条）。缺口登记：NFKC 折叠（需 Unicode 依赖）。
- **PR-3（invalidate 传播，AC4）已开**：PR #16（draft，堆叠在 #14 之上），分支 `feat/r5-invalidate`
  （commit `ed4a696`）。交付：`FactStore::scan`（真值面枚举，双后端）、`invalidate_source(session, event, at)`
  （引用匹配 → 闭 `valid_to` + revision bump，最小爆炸半径）；顺带校正 `pending.is_empty()` 分支丢计数的
  既有缺陷。测试坐标：`tests/write_gateway.rs::invalidate_source_closes_citing_facts_and_stays_closed`
  （含清水位重驱动断言）、`tests/{scope_prefilter,turso_facts}.rs::scan_returns_every_admitted_row_in_id_order`。
  登记推迟：`DependsOn` 边级联（该边暂无生产者）。
- **PR-4（stale 读时派生 + 读侧谓词补口，AC6）已开**：PR #17（draft，堆叠链 #14 → #16 → #17），
  分支 `feat/r5-stale`（commit `b043084`）。交付：`RecallOptions.staleness: Option<StaleRule>`
  （读时派生，`None`=R5 前行为；`now_secs` 调用方注入）+ 材料化改用 `Fact::is_current()`
  （收口「closed/Rejected 行经派生臂漏进 L0」的读视图缺口）。测试坐标：
  `tests/memory_recall.rs::ac6_staleness_is_read_time_and_revalidates_on_corroboration`
  （旧行扣下 / 新行照过 / closed 行两臂仍不入 L0 / 二来源恢复且零写入）。
- **R5 四片全部落地（PR #14/#15/#16/#17，链式堆叠）**：合并顺序 = #14 → rebase #15/#16 → 合并 →
  rebase #17 → 合并；每片本地全量 36 target 绿 + clippy 0 + fmt 干净，远端 CI 各自绿。
- 登记偏差：NFKC 折叠（PR-2）；`DependsOn` 边级联（PR-3）；AC1 派生索引重建口径（重建接缝未建）。
- **合并完成（2026-10-16）**：四片按序 squash 进 main —— #14 `911a6c6`、#15 `25c6e8a`、#16 `04b5332`
  （含 rebase 冲突解决与「坏提交未带修复」事件的更正 `c4cf09a`）、#17 `53ff0b2`（cherry-pick 重建分支以避开
  squash 后的重复重放）。main push CI 三次全绿（19m37s / 7m18s / 6m49s）；远端与本地 R5 分支已删、
  worktree 已清。
- 下一动作（R5 收尾）：真实重建烟测——更正 → 重建派生索引 → 更正仍生效（AC1 的派生索引口径）。
