# AUDIT DASHBOARD

- novel: `novel1`
- concept cards: `2`
- plan files: `2`
- draft chapters: `2`

## Concept
- total warnings: `45`
- `characters` cards=`1` warnings=`29` top=`陆沉.md` (`29` | missing_section_field x15, missing_heading x7, missing_field x6)
- `items` cards=`1` warnings=`16` top=`信号枪.md` (`16` | missing_field x6, missing_section_field x6, missing_heading x3)
- summary files:
  - `tests/fixtures/consistency/novel1/concept/card-stats/characters/SUMMARY.md`
  - `tests/fixtures/consistency/novel1/concept/card-stats/items/SUMMARY.md`

## Plans
- total warnings: `16`
- chapter functions: unclear x1
- ending functions: unclear x1
- story function trends:
  - `story1` chapter_flow=`unclear` ending_flow=`unclear`
- `chapter-plan` files=`1` warnings=`3` top=`ch05.md` (`3` | missing_heading x1, missing_chapter_function x1, missing_scene x1)
- `story-plan` files=`1` warnings=`13` top=`story1-plan.md` (`13` | missing_heading x10, thin_events x1, missing_loads x1)
- summary files:
  - `tests/fixtures/consistency/novel1/chapter-plan-stats/SUMMARY.md`
  - `tests/fixtures/consistency/novel1/story-plan-stats/SUMMARY.md`

## Drafts
- total warn sections: `2`
- workspace templates: sentence_length::短句密度 x2
- deposition targets: configs/rules/review.yaml#draft.template_rules x2
- plan-draft alignment: 无
- `.` chapters=`2` warn_sections=`2` top=`story1-ch01.md` (`1` | 短句密度 x0) fatigue=`短句/极短句 x0`
  gate=`PASS x2` recommendation=`retain x2` avg_axes=`一致性准备度 5.0, 句式弹性 4.5, 场景色调稳定 5.0, 对白情感与转轴 5.0, 张力与紧凑度 4.0, 结构完成度 4.5, 视角与判断稳定 4.5, 重复控制 5.0`
  narrative=`scene:mixed x2 tone:pressure x2 emotion:无 speakers:无` templates=`sentence_length::短句密度 x2` alignment=`无` ending_signals=`未归类 x1 关系转向 x1`
- mirror stats:
  - `tests/fixtures/consistency/novel1/draft-stats`
- scorecard summaries:
  - `tests/fixtures/consistency/novel1/draft-stats/scorecards/SUMMARY.md`
- review kits:
  - `tests/fixtures/consistency/novel1/draft-stats/review-kit/SUMMARY.md`
- template backlogs:
  - `tests/fixtures/consistency/novel1/draft-stats/template-backlog/SUMMARY.md`
- template research:
  - `tests/fixtures/consistency/novel1/draft-stats/TEMPLATE_RESEARCH.md`
- template catalog:
  - `tests/fixtures/consistency/novel1/draft-stats/template-catalog/SUMMARY.md`
  - `tests/fixtures/consistency/novel1/draft-stats/template-catalog/CATALOG.json`

## Consistency
- index: `tests/fixtures/consistency/novel1/research/consistency/consistency.sqlite3`
- feedback log: `tests/fixtures/consistency/novel1/research/consistency/review-feedback.jsonl` entries=`1`
- feedback decisions: `watch` x1
- feedback facets: `state_progression::watch` x1
- pending review: `3`
- feedback backlog:
  - `novel1/research/consistency/review-feedback.jsonl` `story1` 仍有 1 条长期待观察反馈，说明这条 Story 的一致性口径还没真正收敛。
- pending samples:
  - `story1` `goal_state_drift` `陆沉` confidence=`high`
  - `story1` `relationship_tone_shift` `陆沉` confidence=`high`
  - `story1` `injury_state_jump` `陆沉` confidence=`medium`
- story alignment:
  - `story1` plan_entities=`2` draft_entities=`2`
- narrative trajectories:
  - `.` 无
  - `story1` goal=陆沉 x1 ; relationship=陆沉 x1 ; state=信号枪 x1 陆沉 x1
    detail=`state:信号枪 story1-ch01 equipment_active:接口/equipment_damaged:裂开/equipment_damaged:失灵 -> story1-ch02 equipment_active:亮起/equipment_active:护住 | state:陆沉 story1-ch01 injury_negative:擦伤/injury_stable:稳住/injury_stable:止住`
- story trajectories:
  - `story1` state=`信号枪 x1 陆沉 x1` goal=`陆沉 x1` relationship=`陆沉 x1`
    sample=`state `信号枪` equipment=裂开,失灵->亮起,接口,护住 | state `陆沉` injury=擦伤->稳住,止住,还能打`
- trajectory details:
  - `story1`
    - `state` `信号枪` equipment=裂开,失灵->亮起,接口,护住 timeline=`story1-ch01 equipment_active:接口/equipment_damaged:裂开/equipment_damaged:失灵 -> story1-ch02 equipment_active:亮起/equipment_active:护住 -> ch05 equipment_active:亮起/equipment_active:接口`
    - `state` `陆沉` injury=擦伤->稳住,止住,还能打 timeline=`story1-ch01 injury_negative:擦伤/injury_stable:稳住/injury_stable:止住`
    - `goal` `陆沉` assigned=交给,任务 ; changed=目标变成 ; completed=收尾 timeline=`story1-ch01 goal_assigned:任务/goal_assigned:交给/goal_changed:目标变成 -> ch05 goal_assigned:交给/goal_completed:收尾`
    - `relationship` `陆沉` close=接住,接应 ; distant=提防,怀疑 timeline=`story1-ch01 relationship_distant:提防/relationship_distant:怀疑 -> story1-ch02 relationship_close:接住/relationship_close:接应`
- relationship pair trajectories: 无
- state tension:
  - `story1` `信号枪` (items) equipment=裂开,失灵 -> 亮起,接口,护住
  - `story1` `陆沉` (characters) injury=擦伤 -> 稳住,止住,还能打
- goal tension:
  - `story1` `陆沉` (characters) assigned=交给,任务 ; changed=目标变成 ; completed=收尾
- relationship tension:
  - `story1` `陆沉` (characters) close=接住,接应 ; distant=提防,怀疑
- conflict candidates:
  - `story1` `equipment_state_jump` `信号枪` (items) confidence=`high` support=`draft_docs=2` equipment=裂开,失灵 -> 亮起,接口,护住
  - `story1` `goal_state_drift` `陆沉` (characters) confidence=`high` support=`draft_docs=1 upstream_docs=1` assigned=交给,任务 ; changed=目标变成 ; completed=收尾
  - `story1` `relationship_tone_shift` `陆沉` (characters) confidence=`high` support=`draft_docs=2` close=接住,接应 ; distant=提防,怀疑
  - `story1` `injury_state_jump` `陆沉` (characters) confidence=`medium` support=`draft_docs=1` injury=擦伤 -> 稳住,止住,还能打

## Suggested Order
1. 先修 `draft` 里 `gate=FAIL`、`recommendation=targeted_rewrite` 的章节，再看 `pairs / triples`
2. 再修 `chapter-plan` 与 `story-plan` 的字段错位和空字段
3. 如果某章评分里 `一致性准备度` 明显偏低，先跑 `sentinel consistency suspects` 再决定是否只是局部误写
4. 如果已经锁定某条 Story，要逐条复核一致性候选，直接跑 `sentinel consistency review-queue novel1 --story storyN`
5. 做完一轮局部复核后，立刻跑 `sentinel consistency feedback-summary novel1 --story storyN` 看这一条 Story 是否开始收敛
6. 最后补 `concept` 缺口，避免下游继续空转
