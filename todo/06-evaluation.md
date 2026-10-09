# R6 评测（MTRAG / LongMemEval / 自建 case + manifest）

**状态**：待实施。
**目标**：用可重放的评测证明记忆层真实有效——多轮对话不退化、多跳可召回、成本/延迟有账；判据先冻结再跑分。
**前置**：[R5](05-reflect.md)验收。
**下一阶段**：无（可选扩展：auto 选择器）。

## 范围与非目标

- 做：评测 manifest（复用 context-compact C08 格式）、MTRAG 子集、LongMemEval 子集、自建 A→B→A case、三轴报告（质量/成本/延迟）。
- 不做：通用 benchmark 平台、训练、把一次跑分当排名。

## 评测集

| 集 | 来源 | 用途 | 许可 |
|---|---|---|---|
| MTRAG 子集 | IBM（2501.03468） | 多轮对话检索退化检测 | 使用前核 |
| LongMemEval 子集 | 公开 benchmark | 长会话记忆召回 | 使用前核 |
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

- R6.T1 manifest 骨架 + 冻结脚本（case/run JSON schema）。
- R6.T2 MTRAG 子集接入（对话切片 → 建库 → 按 case 重放）。
- R6.T3 LongMemEval 子集接入。
- R6.T4 自建 A→B→A 与标识符集。
- R6.T5 跑分与三轴报告（含失败样本如实列出）。
- R6.T6 CI gate：判据冻结后设为回归门槛；指标回退即红。

## 验收

- [ ] R6.AC1 同一 case 可重放：现场重新生成，不看旧产物；evidence_refs 可回溯。
- [ ] R6.AC2 四类评测集各有实际结果；失败样本如实报告。
- [ ] R6.AC3 三轴报告含 1/10/50 规模数据与冷/热延迟。
- [ ] R6.AC4 MTRAG 多轮场景无显著退化（阈值在冻结步骤前定）。
- [ ] R6.AC5 判据冻结先于跑分（时间戳可证）；冻结后未改。
- [ ] R6.AC6 CI gate 生效：人工注入一次指标回退（或对旧版本跑）验证会红。

## 验证与烟测

本地不跑全量 benchmark；CI 跑夹具级回归；真实评测在授权隔离环境，费用上限先定。

## 接续记录

- 已完成：无。
- 未完成：T1–T6。
- 下一动作：R5 验收后先写 manifest schema 与冻结脚本。
