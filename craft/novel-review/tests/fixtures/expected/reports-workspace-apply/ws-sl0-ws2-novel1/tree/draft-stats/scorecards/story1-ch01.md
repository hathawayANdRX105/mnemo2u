# story1-ch01 Review Scorecard

- source: `tests/fixtures/consistency/novel1/drafts/story1-ch01.md`
- gate: `PASS`
- priority: `P2`
- recommendation: `retain`
- note: 当前章可保留主体结构，优先微调个别硬项，不要为了清零统计把文气磨平。

## Axis Scores
| 维度 | 分数 | 说明 |
|---|---:|---|
| 重复控制 | `5` | 综合硬警告、句式疲劳、局部点名密度与句首骨架重复。 |
| 句式弹性 | `4` | 观察短句、并列分句、AA/BB 节奏和分句前缀，判断是节奏还是手癖。 |
| 对白情感与转轴 | `5` | 看对白是否有动作、环境、第三方或设备转轴，而不是长时间互顶。 |
| 场景色调稳定 | `5` | 暂时用章末模板、修饰压力和局部疲劳窗口做代理指标，后续再接更细的色调分类。 |
| 张力与紧凑度 | `4` | 用句长、对白互顶和转轴缺口粗看战斗/冲突段是否只是快而不紧。 |
| 视角与判断稳定 | `5` | 当前主要用判断句上下文、语料偏移和高优先提醒做代理，先拦旁白抢跑与说明过重。 |
| 一致性准备度 | `5` | 看当前章与同书语料的偏离程度，以及当前 story 的一致性候选是否已被复核、确认或仍待处理。 |
| 结构完成度 | `5` | 暂用高优先提醒、局部高压窗口和总体告警量做代理，后续再接 Scene/章末功能分析。 |

## Hard Gates
- warn_sections=`1`
- hard_flags=`1`
- P1 reminders=`0`
- fatigue_windows=`0`
- tracked_term_windows=`0`

## Bonus Candidates
- `章末收束未模板化`：章末没有落入现有意象/流程词模板，说明结尾功能有继续发展的空间。

## Narrative Signals
- scene_blocks=`1` dominant_role=`mixed` dominance_ratio=`1.0` switches=`0`
- dialogue_emotion=`neutral` ratio=`0.0` shifts=`0`
- character_voice=`none` speakers=`0` coverage=`0.0` warn=`False`
- tone=`pressure` stable_ratio=`1.0` tone_switches=`0`
- battle_sequences=`1` result_ratio=`0.0`
- viewpoint_anchor=`他` switches=`0` overlaps=`0`
- ending_signal=`未归类`

## Consistency Snapshot
- 无一致性反馈快照

## Plan Alignment Snapshot
- 无 plan-draft 对齐快照

## Priority Fixes
- 无 P1 项

## Top Hard Flags
- `sentence_lengths` `短句连发` x1：连续短句会把叙述切成机械节拍；类型：动作 x1，其他 x4；建议：先判断这些短句是否都必要；只保留一个节奏点，其余展开。 样例：信号枪裂开了，接口失灵 | 陆沉稳住伤势，血止住了 | 阿沉擦伤缠住，但还能打 | 陆沉提防阿沉，怀疑他泄密 | 任务交给陆沉，目标变成先走

## Follow-up
- 如果是 `WATCH` 或 `FAIL`，先读对应 `profiles/` 目录里的句式画像，再决定是删词、拆句还是重写段落。
- 如果同一角色、地点、装备或称谓在这章显得摇摆，先跑本章快照里的 `review_queue`，再决定是否补 `feedback-add`。
- 如果 `recommendation` 已接近 `targeted_rewrite`，先回查 `chapter-plan` 和 `story-plan`，不要只在正文层补丁。

