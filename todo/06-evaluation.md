# R6 评测（MTRAG / LongMemEval / 自建 case + manifest）

**状态**：已全部落地并合并（2026-10-16）。T1/T4/T5/T6 随 PR #11（`172c15d`）；T2/T3（MTRAG/LongMemEval 外部集）随 PR #12 squash `388b219` 落地；分支 `feat/r6-and-gaps` 已删。
**目标**：用可重放的评测证明记忆层真实有效——多轮对话不退化、多跳可召回、成本/延迟有账；判据先冻结再跑分。
**前置**：[R5](05-reflect.md)验收（T1/T4/T5/T6 已提前落地，不回溯阻塞）。
**下一阶段**：无（可选扩展：auto 选择器）。

## 许可核实（2026-10-15，原「使用前核」已核）

- **MTRAG**：仓 LICENSE = Apache-2.0（`todo/refs/mt-rag-benchmark/LICENSE` 核过）；README 要求引用 katsis2025mtrag（TACL 2025）。语料（ClapNQ/FiQA/Govt/Cloud）来自第三方上游——**只切 MTRAG 自有 human 对话与问题切片，不搬第三方语料全文**。
- **LongMemEval**：MIT（GitHub license API `spdx_id: MIT`，Copyright (c) 2024 Di Hu；数据包为仓外发布物，非 git 树内容）——夹具需保留版权声明。

## 范围与非目标

- 做：评测 manifest（复用 context-compact C08 格式）、MTRAG 子集、LongMemEval 子集、自建 A→B→A case、三轴报告（质量/成本/延迟）。
- 不做：通用 benchmark 平台、训练、把一次跑分当排名。

## 评测集

| 集 | 来源 | 用途 | 许可 |
|---|---|---|---|
| MTRAG 子集 | IBM（2501.03468） | 多轮对话检索退化检测 | Apache-2.0（见上核实；引用 katsis2025mtrag） |
| LongMemEval 子集 | xiaowu0162/LongMemEval | 长会话记忆召回 | MIT（见上核实；保留版权声明） |
| 自建 A→B→A | 本库夹具 | 焦点切换后回查早期证据 | 自有 |
| 精确标识符集 | 本库夹具 | 路径/错误码/符号召回 | 自有 |

## Manifest 契约（复用 C08）

- `case`：case_id、snapshot_id、task/focus 状态、must_keep、expected_source（SourceRef 列表）、target_action、success_conditions、repetitions。
- `run`：code_revision、method/config 版本、execution/summary/jev 实际模型、requested/effective、budget_source、起止时间、error、evidence_refs、资源采样。
- **冻结顺序**：先冻 success_conditions 与阈值 → 再冻 case 快照 → 最后跑。跑完不改判据。

## 三轴报告

1. **质量**：续作成功率、事实/约束保留、漏召回率、错误引用率；Jev 评审仅辅助，人工核对判据为准。
2. **成本**：Jev 调用数/用量、embedding 调用、每查询 token；服务不报 usage 标未知。
3. **延迟**：本地三臂 P50/P95；含 Jev 的网络段单列；万级条目夹具 + 1/10/50 会话三种规模。

## 任务

- [x] R6.T1 manifest 骨架 + 冻结脚本（case/run JSON schema）。（PR #11；`Report::manifest_json` + `snapshot_id` 冻结哈希。）
- [x] R6.T2 MTRAG 子集接入（对话切片 → 建库 → 按 case 重放）。（`src/eval/external.rs::load_mtrag` + `tests/fixtures/mtrag_subset.json`（Apache-2.0 切片、16 message、provenance 必填校验）+ `tests/eval_external.rs`；`388b219`。）
- [x] R6.T3 LongMemEval 子集接入。（`load_longmemeval` + `tests/fixtures/longmemeval_subset.json`（MIT、copyright 保留、revision 记录）；`388b219`。）
- [x] R6.T4 自建 A→B→A 与标识符集。（`src/eval/cases.rs` + `tests/eval_gate.rs`；PR #11。）
- [x] R6.T5 跑分与三轴报告（含失败样本如实列出）。（`Report::from_cases` 三轴 + `Failure` 枚举如实列失败；PR #11。）
- [x] R6.T6 CI gate：判据冻结后设为回归门槛；指标回退即红。（`tests/eval_gate.rs` 跑在 CI `cargo test --all-targets`；PR #11。）

## 验收

- [x] R6.AC1 同一 case 可重放：现场重新生成，不看旧产物；evidence_refs 可回溯。（`run_case` 现场重跑 + `snapshot_id` 哈希稳定；`tests/eval_gate.rs::snapshots_are_stable_and_corpus_edits_change_them`；`Report::manifest_json` 带 code_revision/started_at/finished_at。随 PR #11。）
- [x] R6.AC2 四类评测集各有实际结果；失败样本如实报告。（自建两集 `tests/eval_gate.rs`；MTRAG/LongMemEval `tests/eval_external.rs` 4 用例（含 forbidden/near-miss 失败样本断言）；`388b219`。）
- [x] R6.AC3 三轴报告含 1/10/50 规模数据与冷/热延迟。（`Report::from_cases` 三轴 + `quality_holds_across_scale` 1/10/50；p50/p95 打印入 CI 日志。延迟为本地热路径（库内热启动），冷启动延迟在库形态无独立面——以每次 fresh workspace 跑 case 计。随 PR #11。）
- [x] R6.AC4 MTRAG 多轮场景无显著退化（阈值在冻结步骤前定）。（MTRAG case 的 `Conditions` 在构造时冻结进 `snapshot_id`；`tests/eval_external.rs::mtrag_case_recalls_gold_evidence` 端到端 `run_case` 过冻结条件；`388b219`。）
- [x] R6.AC5 判据冻结先于跑分（时间戳可证）；冻结后未改。（`snapshot_id` 哈希含 `Conditions`，改阈值即轮换快照，钉死于 `eval_gate.rs` 的 threshold-edit 用例。随 PR #11。）
- [x] R6.AC6 CI gate 生效：人工注入一次指标回退（或对旧版本跑）验证会红。（`tests/eval_gate.rs` 跑在 `cargo test --all-targets`；指标回退=条件不满足→`passed()=false`→测试红。阈值放宽路径已由 AC5 的快照轮换钉住——放宽必须显式改码并留痕。随 PR #11。）

## 验证与烟测

本地不跑全量 benchmark；CI 跑夹具级回归；真实评测在授权隔离环境，费用上限先定。

## 接续记录

- 已完成：T1/T4/T5/T6（PR #11，`172c15d`）+ T2/T3（外部集，PR #12 squash `388b219`，2026-10-16 合并）——六任务全落，AC1–AC6 全勾（证据坐标见 §验收）。
- 未完成：无（登记项：MTRAG 第三方语料正文不入库的口径见 §许可核实；AC3 冷启动口径=每次 fresh workspace）。
- 下一动作：无（R6 收尾）。R5 四片已合并（#14–#17，2026-10-16）；真实端点冒烟已跑（#18 修复后 CLI 全链通过）。
