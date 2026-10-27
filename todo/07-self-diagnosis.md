# R7 自我诊断（S1 归因；S2 旋钮登记；S3 搜索闭环；S3.5 用例地形，均已合并）

**状态**：S1 已实施并合并（PR #20 squash `bd2c621`，2026-10-17：`src/diag` + 12 测试 + CI smoke step；jev 3 轮 patch 评分 composite ≥0.80、`jev_gate` 5/5 claims verified / 0 contradicted）。**S2 已合并**（PR #21 squash `d42251f`，2026-10-17：`src/tune` + `tests/tune_knobs.rs` 10 测 + `tests/eval_baseline.rs` + 基线 fixture + `examples/tune_probe.rs` + CI smoke step；本地 fmt/check 干净，CI run [39046149819](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39046149819) 全绿）。调查与方案来源：[design/08-self-diagnosis](design/08-self-diagnosis.md)（外部 10 篇主源逐篇核验）。
**目标**：诊断成为读路径的一等输出——对一次 miss 回答「丢在哪一层」，并给出机器可消费的臂健康行；与 `degraded` 同族，不靠外挂脚本。
**前置**：[R3](03-retrieval.md)（臂序/降级契约）、[R5](05-reflect.md)（读时过滤语义）、[R6](06-evaluation.md)（冻结契约与 CI 门文化）。
### jev 评审记录（2026-10-18，补齐 R7 三轮缺失的 jev 口径）

调用方式：`jev_gate` / `jev_review`，model `decision-model-preview`，provider `compatible`，端点 `http://127.0.0.1:8001/v1/systemone`（本地 jev-adapter，透传通义 MaaS）。

| PR | 主题 | verification | composite | 限制项 |
|---|---|---|---|---|
| #22 `ef93ba8` | S3 搜索闭环 + 四条接受规则 | **10 verified / 0 contradicted / 0 unsupported / 0 needs_review**，conf 0.95–0.98 | 0.875 | `test_gap` 0.60（conf 0.70） |
| #23+#24 `bde4d77`+`969877b` | S3.5 `Case::edges` + 未饱和调参集 + holdout 6 query | **11 verified / 0 contradicted / 0 unsupported**，conf 0.96–0.98，`action: auto` | 0.902 | `test_gap` 0.47（conf 0.77）、`blast_radius` 0.56 |
| #25 `62ab143` | `KnobRegistry::set` 错误路径测试补齐 | （PR #25 本身是 #21/#22 `test_gap` 的闭环，未单独跑 gate） | — | — |

另跑一次针对性 `jev_review`（mode per-file，对象是 `src/eval/mod.rs` 的 `CaseEdge` + `run_case_with` 边写入块，与 `src/index/gateway.rs` 的 `fact_id` 可见性）：两文件 `correctness` 均 **1.98**（conf 0.99）、`spec_match` 均 **1.96**（conf 0.98），composite 0.861、safe_to_apply 0.89；唯一限制项同为 `test_gap`（1.02，conf 0.51）。

**`test_gap` 是真缺口，已闭环**：`KnobRegistry::set`（PR #22 引入）有越界/`Unset`/未知 id 三条错误路径与一条成功路径，此前**零覆盖**——搜索自己的边界检查在 `evaluate` 里先跑，`set` 的拒绝只能被跳过该守卫的调用方触达。PR #25（squash `62ab143`）补了 2 条测试，`tests/tune_search_set.rs` 7→9，纯测试补齐、src 零改动。

**本轮三个 PR 均 0 contradicted / 0 unsupported**，即 jev 未否证任何一条实现声明；`test_gap` 指向的覆盖缺口已修。三个 PR 在此之前没有 jev 记录（`todo/handoff/r7-s3-set.md` §5 曾记为「未跑 jev 评审」），本段补齐。

### 闭环 `test_gap` 时顺带查出并修掉的缺陷（PR #27 `7b418eb`）

`KnobRegistry::set` 只查边界就返回，而 `KnobBounds::admits` 把任何变体都读成一个数——所以 `F64(2.0)` 写进行 `u8` 的 `recall.graph_hops` **在窗口内、会被接受**，直到 `recall_knobs()` 才以 `WrongKind` 失败。报错指向那一行，而引起问题的写入早已不在调用栈里。

修法：种类不符 → `WrongKind`；无值的行（四个按调用方预算的行本来就持 `Unset`，没有种类可比）→ `UnsetKnob`，不替它猜一个。

**搜索为何不受影响**（这次改动的安全边界）：`Search::directions` 的分支按 `Usize/U8/U32/F64` 各自只产同变体的 ±1，`Unset` 直接返回空——它产不出种类不符的步，也产不出针对无值行的步。全仓 `set` 的非测试调用方只有 `src/tune/search.rs` 两处（`evaluate` 的候选副本与 `commit`）。CI run 39124063694 实测 probe 接受结果与修复前完全一致（`1 of 16`、`graph_hops 2→3`）。

后续三条（S3.5.4）把这件事做扎实，都只碰 `src/tune/mod.rs` 的 doc 与 `tests/tune_search_set.rs`：

- **PR #28 `9c7cf79`**：无值行拒绝的断言从 `recall.top_k` 一个扩展到四个（`top_k`、`per_arm_limit`、`recall.stale.max_age_secs`、`recall.stale.min_sessions`），并逐个断言被拒后仍是 `Unset`。理由是：只点一个例子的话，这个类以后扩容、或其中一行悄悄获得默认值而不再属于这个类，测试照样绿——它保护的是例子而不是规则。
- **PR #29 `812f3c4`**：给那个判断取名 `KnobValue::same_kind`，doc 写清「为何边界检查不够」（`as_f64` 把每个变体压成一个数）。此前它是埋在 `set` 里的一个裸 `mem::discriminant`，读者得从两行代码反推它在守什么性质。纯重构，谓词没变，CI 实测 probe 输出逐字一致。
- **PR #30 `1e86719`**：继续查 `admits` 的其余调用面时发现 `admits(Unset) == true` 是个没说出口的承重墙。第一眼像个洞（一个 `Unset` 岂不是能穿过窗口），但它正是让基线不变量 `knob.bounds.admits(&knob.current)` 在那四行无值行上成立的东西。doc 补上它承载什么、以及由此推出的结论（`admits` 单独不能守护写入，故 `set` 先查种类再查边界），并加测试把不变量前后两半都钉住。无逻辑改动。

**下一阶段**：S3.6（给 `Case` 的边加更多关系类型，解锁 `rel_support` 等其余旋钮，见本文件 S3.5 消融数据之后）→ S4 语义档 + 飞轮（D3 `judge_checks` + PPI 校准；T4 CaseMiner）——见 design/08 §五。

## S2 交付（已合并 `d42251f`，PR #21）

- 范围：`tune::KnobRegistry`（12 旋钮：id/layer/bounds/rationale）+ `knobs_for(stage)` 把 S1 归因映射到可调旋钮的机械保证（无 wildcard arm，新增 stage 编译失败）+ `scoreboard_json`（质量/成本精确环比，延迟只记录不环比）+ `KnobRegistry::recall_knobs()` 作为 registry→注入结构的桥。
- 关键坐标：`RecallKnobs`（`src/query/facts.rs`，7 字段，`Default` = S2 前常量）与 `MemoryRecall::recall_with`（注入缝；`recall` 签名不变并委托，40 个既有调用点零改动）。
- 验收（已勾）：默认装配零行为变更（`recall_with_defaults_is_the_recall_path` 在真实 Turso 存储上逐候选比对；`a_moved_knob_reaches_the_read_path` 证明旋钮真的接通）、基线 JSON 入库（CI 真实产物，`1.0` 等数字类型由 Rust 侧 serde 写出）、CI 对比红（`baseline_matches`，比较前剥掉所有 latency）、smoke step（`examples/tune_probe.rs`）。
- **遗留（本轮登记不动）**：`src/eval/external.rs:68` 的 `L0_SUMMARY_CHARS=200` 是 `SUMMARY_CHARS` 的镜像副本（不在白名单，待 S4 动外部集时折叠）；`src/config.rs:244-249` 与 rag.rs parity 常量的重复属配置面，待 RAG 面调优轮处理。

## 契约（S1 冻结项）

1. **入口**：`diag::attribute(recall, scope, query_text, query_vec, opts, marker, outcome)`——诊断**一次** recall 的产出；`outcome` 必须与其余参数同源，否则报 `Indeterminate`。
2. **stage 词表**（冻结）：`Delivered` / `NotCaptured` / `NotCurrent{Superseded|ClosedValidTo|Rejected}` / `InvisibleToScope` / `StaleFiltered` / `AllArmsMissed` / `RankedOut{position,top_k}` / `DemotedByJudge{pre_rank,top_k}` / `JudgeCleared` / `Indeterminate`；每条附 typed `Evidence`（`TruthScan`/`Captured`/`ScopeCheck`/`StaleRule`/`ArmProbe`/`RetrievalProbe`/`TopK`/`JudgeRun`）。
3. **探测纪律**：重放=同 store、judge 剥离、**沿用调用方的 per_arm_limit/rrf_k**；`top_k` 提升至 `usize::MAX` 仅用于暴露截断位；staleness 只翻转一次、专供 `StaleFiltered` 判定；向量臂无查询向量 → `asked:false`（不是失败）。
4. **不变量**：诊断只读（不写任何 store、不调模型、零外发）；跨 workspace 不探测（隔离红线）；`Indeterminate` 是诚实标签，禁止兜底猜测。
5. **健康行**：`diag::health(outcome, top_k)`——`exists`/候选数/`saturated`/`degraded`/各臂供给与独占计数；只读 outcome，不触 store。
6. **共享常量**：`JEV_CANDIDATE_CAP` / `SEEDS_PER_ARM` / `GRAPH_HOPS` 由 `query::facts` 以 `pub(crate)` 共享给 `diag`（单一真相源，禁复制粘贴）。

## 验收（S1）

- [x] stage 词表逐项测试（12 条，覆盖全部 stage + health + 真后端端到端），每测注明「哪种 bug 会红」。（`tests/diag_attribution.rs`，PR #20 `bd2c621`；CI `diag_attribution` 12/12。）
- [x] 真后端集成测试（Turso 真值 + FTS + 邻接）至少一条。（`real_backends_deliver_and_report_uncaptured`。）
- [x] CI：`cargo test --all-targets` 全绿 + `examples/diag_probe` smoke step 绿。（run [39020297890](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39020297890)；smoke 输出 `RankedOut{0,0}` / `NotCaptured`。）
- [x] 无 recall 行为变更（`query/facts.rs` 仅常量可视性 + `ArmName` derive）。（jev_gate claim 3 verified @0.95。）

## 接续记录

- 2026-10-17：S1 开工→合并一条龙。任务书 [handoff/r7-diag](handoff/r7-diag.md)；实现 PR #20 squash **`bd2c621`**（6 文件 +1252/-6；本地 fmt/check 干净，测试按仓库规则在 CI 执行）。jev 辅助：3 轮 patch 评分（composite 0.831/0.803/0.872，correctness ≥1.89）+ gate 5/5 claims verified、0 contradicted（PR #20 comment 留档）。下一动作：S2（旋钮外化 + scoreboard 基线），见 design/08 §五。
- 2026-10-17（交接）：S2 任务书已写——**[handoff/r7-s2](handoff/r7-s2.md)**（新会话按 §0–§10 执行；基线 `bd2c621`，worktree `.wt/r7-s2`）。
- 2026-10-17（深夜）：**S2 实施并合并**。任务书 [handoff/r7-s2](handoff/r7-s2.md)；实现 PR **#21** squash **`d42251f`**（8 文件 +1458/-19，分 3 个 commit：主实现 `dbd53f8`、基线回填 `4113d55`、JSON 数字类型修正 `bde2362`）。worktree `.wt/r7-s2` 与分支 `feat/r7-s2` 已清理，本地 main 已 ff 到 `d42251f`。CI run [39046149819](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39046149819) 全绿（fmt / clippy / test / smoke）；`tests/tune_knobs.rs` 10/10，`tests/eval_baseline.rs::baseline_matches` ok。§2 四问回执：PR #21 正文 + 本地 `handoff/r7-s2-receipt.md`。jev 本轮未跑（PR #21 无 jev 评审记录；S1 的 jev 口径仍有效）。下一动作：**S3 搜索闭环**（T3 SearchStepper：坐标下降 + 接受规则 噪声地板 ∧ holdout 不退化 ∧ wrong=0 保持 ∧ ratchet），注入面已就绪（`tune::default_registry().recall_knobs()` → `MemoryRecall::recall_with`）。
- 2026-10-18：**S3 实施完毕，PR #22 待合并**（CI run [39060381081](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39060381081) 全绿；`tests/tune_search.rs` 13/13；任务书 [handoff/r7-s3](handoff/r7-s3.md)，§2 回执即 PR #22 正文）。产物：`src/tune/search.rs`（`Search`/`Step`/`Verdict`，坐标下降 + 四条接受规则）、`src/eval/mod.rs::run_case_with` 注入缝（`run_case` 委托，默认装配零行为变更）、`src/tune/mod.rs::KnobRegistry::set`、`examples/tune_search_probe.rs` + CI smoke step。

### S3 第一轮单旋钮消融数据（CI run 39060381081，probe 输出）

调参集 `cases::all(0)` = 6 query（focus-switch 1 + identifier 5），holdout `external_all(0)` = 2 query（mtrag 1 + longmemeval 1），噪声地板 = 1.0/6 ≈ 0.1667。**默认装配在两个集合上均 recall=1.0、miss=0.0、wrong=0.0**——即已饱和，无头部空间。

| # | knob | from → to | 裁决 | gain | holdout 系数 |
|---|---|---|---|---|---|
| 1 | recall.graph_hops | U8(2) → U8(1) | RejectNoiseFloor | 0.0 | 不退化 |
| 2 | recall.graph_hops | U8(2) → U8(3) | RejectNoiseFloor | 0.0 | 不退化 |
| 3 | recall.jev_candidate_cap | 250 → 249 | RejectNoiseFloor | 0.0 | 不退化 |
| 4 | recall.jev_candidate_cap | 250 → 251 | RejectNoiseFloor | 0.0 | 不退化 |
| 5 | recall.rel_correction | 1.5 → 1.35 | RejectNoiseFloor | 0.0 | 不退化 |
| 6 | recall.rel_correction | 1.5 → 1.65 | RejectNoiseFloor | 0.0 | 不退化 |
| 7 | recall.rel_support | 1.2 → 1.08 | RejectNoiseFloor | 0.0 | 不退化 |
| 8 | recall.rel_support | 1.2 → 1.32 | RejectNoiseFloor | 0.0 | 不退化 |
| 9 | recall.rerank_near_zero | 0.05 → 0.0 | RejectNoiseFloor | 0.0 | 不退化 |
| 10 | recall.rerank_near_zero | 0.05 → 0.15 | RejectNoiseFloor | 0.0 | 不退化 |
| 11 | recall.seeds_per_arm | 10 → 9 | RejectNoiseFloor | 0.0 | 不退化 |
| 12 | recall.seeds_per_arm | 10 → 11 | RejectNoiseFloor | 0.0 | 不退化 |
| 13 | recall.summary_chars | 200 → 199 | RejectNoiseFloor | 0.0 | 不退化 |
| 14 | recall.summary_chars | 200 → 201 | RejectNoiseFloor | 0.0 | 不退化 |

**结论**：14 个方向全部被噪声地板拒绝，0 个接受，搜索在第 14 步收敛（`propose()` 返回 `None`），probe 输出 "no accepted step: all 14 proposed direction(s) were rejected, so the default assembly stands and no gain was faked"。**这不是失败，是这套用例集的真实状态**：两个集合都已满分，任何 knob 移动都不可能产生增益；接受规则按设计拒绝，而不是为了绿灯假造一个增益。holdout 系数全程为 0（不退化）。

**下一轮要做的事**：要让搜索有东西可接受，需要**未饱和的用例集**。已立项为 S3.5，任务书与阻塞结论见 [handoff/r7-s3-set](handoff/r7-s3-set.md)。

**S3.5 已合并：PR #23 squash `bde4d77` + PR #24 squash `969877b`（2026-10-18）**，CI run [39104465937](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39104465937) 与 [39106496528](https://github.com/hathawayANdRX105/mnemo2u/actions/runs/39106496528) 全绿。PR #24 把 holdout 从 4 query 扩到 6 query（原任务书要求 ≥6，S3.5 交付时只到 4）。此前确认的阻塞与解法：`Case` 产不出边（无 edge 字段 + `judge: None`）→ 除 `summary_chars` 外所有旋钮恒为零作用，而 `summary_chars` 每步 ±1 只改变被比较文本的一个字符、只能揭示长度 1 的 marker。**解法是给 `Case` 加 `edges`（按轮次位置声明）**，`graph_hop_case` 的 marker 放在第 3 跳，`graph_hops: 2→3` 一步命中。

### S3.5 第一轮有地形后的消融数据（CI run 39104465937）

调参集 = `summary_cut_all(0)`（2 case / 2 query，地板 0.5）；holdout = `held_out_case`（4 query）+ 外部两切片（2 query）= 6 query。默认装配在调参集上 recall=0.0、miss=1.0、wrong=0.0（headroom），holdout 上 recall=1.0。

| # | knob | from → to | 裁决 |
|---|---|---|---|
| 1 | graph_hops | 2 → 1 | RejectNoiseFloor（gain 0） |
| 2 | **graph_hops** | **2 → 3** | **Accept（gain 1）** |
| 3 | graph_hops | 3 → 2 | RejectRegression（tuning 集 frozen 条件被破坏） |
| 4 | graph_hops | 3 → 4 | RejectNoiseFloor（gain 0） |
| 5–16 | jev_candidate_cap / rel_correction / rel_support / rerank_near_zero / seeds_per_arm / summary_chars | ±1 | RejectNoiseFloor（gain 0） |

**16 个方向接受 1 个**；`ratchet -> recall.graph_hops: U8(2) -> U8(3)`；accepted tuning recall 0.0 → 1.0、wrong=0.0。**step 3 是棘轮的证据**：接受后往回走立刻被拒为回归。holdout 全程 recall=1.0 不退化。`tests/tune_search_set.rs` 7/7（两个 CI run 均如此）。

**下一轮**：地形仍然只有 `graph_hops` 一个方向可达（其余旋钮仍需 judge 或更多边类型才能起作用）。要让搜索有多条路可走，下一步是给 `Case` 的边加 `DependsOn`/`MentionsFile` 等类型的用例（解锁 `rel_support`），或让 harness 收 judge（解锁 `rel_correction`/`rerank_near_zero`）。
