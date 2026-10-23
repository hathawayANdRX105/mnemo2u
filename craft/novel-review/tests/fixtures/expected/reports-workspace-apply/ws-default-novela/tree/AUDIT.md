# AUDIT DASHBOARD

- novel: `novel-a`
- concept cards: `0`
- plan files: `0`
- draft chapters: `4`

## Concept
- 无概念卡目录

## Plans
- 无大纲目录

## Drafts
- total warn sections: `54`
- workspace templates: tracked_term::终端 x4 patterns::把字操作句 x4 punctuation::？ x4 sentence_length::短句密度 x4 tracked_term::一下 x3 ba_operation_context::动作操作 x3
- deposition targets: configs/rules/review.yaml#draft.template_rules x37 configs/rules/review.yaml#draft.tracked_terms x19 skills/review-guide.md x5 novel1/rules/draft.md x3
- plan-draft alignment: 无
- `arc1/story1` chapters=`2` warn_sections=`30` top=`ch01-信号.md` (`16` | 终端 x5, 屏幕 x1, 抬头 x1) fatigue=`像/活像比喻 x1, 对白乒乓 x2, 局部疲劳窗口 x6, 把字操作句 x5, 短句/极短句 x32, 章末模板 x7, 视角锚点 x0, 角色名/他她起手 x15, 高频词/点名册 x18, 黏糊词/弱判断 x1`
  gate=`FAIL x2` recommendation=`targeted_rewrite x2` avg_axes=`一致性准备度 2.5, 句式弹性 4.0, 场景色调稳定 2.0, 对白情感与转轴 4.0, 张力与紧凑度 3.0, 结构完成度 2.0, 视角与判断稳定 3.0, 重复控制 1.0`
  narrative=`scene:mixed x2 tone:cold x1 quiet x1 emotion:无 speakers:她 x4` templates=`tracked_term::终端 x2 tracked_term::冷光 x2 tracked_term::慢慢 x2 patterns::把字操作句 x2` alignment=`无` ending_signals=`意象压轴 x1 系统流程 x1`
- `arc1/story2` chapters=`2` warn_sections=`24` top=`ch03-灰港.md` (`13` | 终端 x3, 一下 x1, 因为 x1) fatigue=`像/活像比喻 x1, 对白乒乓 x1, 局部疲劳窗口 x6, 把字操作句 x6, 短句/极短句 x44, 章末模板 x3, 线索面板句 x1, 视角锚点 x0, 角色名/他她起手 x19, 高频词/点名册 x4`
  gate=`WATCH x2` recommendation=`light_revise x2` avg_axes=`一致性准备度 3.5, 句式弹性 4.0, 场景色调稳定 2.5, 对白情感与转轴 5.0, 张力与紧凑度 3.5, 结构完成度 3.0, 视角与判断稳定 3.0, 重复控制 1.0`
  narrative=`scene:dialogue x1 mixed x1 tone:grime x2 emotion:无 speakers:她 x7 他 x4` templates=`tracked_term::终端 x2 tracked_term::一下 x2 patterns::线索面板词 x2 patterns::把字操作句 x2` alignment=`无` ending_signals=`意象压轴 x2`
  trend=`ending_signal_runs=意象压轴 x2; convergence=意象压轴+tone:grime x2`
  ending_signal_flow=`意象压轴 -> 意象压轴`
- mirror stats:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats`
- scorecard summaries:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/scorecards/SUMMARY.md`
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story2/scorecards/SUMMARY.md`
- review kits:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/review-kit/SUMMARY.md`
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story2/review-kit/SUMMARY.md`
- template backlogs:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/template-backlog/SUMMARY.md`
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story2/template-backlog/SUMMARY.md`
- template research:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/TEMPLATE_RESEARCH.md`
- template catalog:
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/template-catalog/SUMMARY.md`
  - `tests/fixtures/catalog-cbk/novel-a/draft-stats/template-catalog/CATALOG.json`
- learning anchors:
  - `keep` `keep_candidates::场面功能有切换` stories=`2` total=`4`
  - `keep` `keep_candidates::局部排比可视作风格点缀` stories=`2` total=`3`

## Consistency
- index: `tests/fixtures/catalog-cbk/novel-a/research/consistency/consistency.sqlite3`
- feedback log: `tests/fixtures/catalog-cbk/novel-a/research/consistency/review-feedback.jsonl` entries=`0`
- feedback decisions: 无
- feedback facets: 无
- pending review: `0`
- feedback backlog: 无
- pending samples: 无
- story alignment: 无
- narrative trajectories:
  - `story1` speakers=她 x4
  - `story2` speakers=她 x7 他 x4
- story trajectories: 无
- trajectory details: 无
- relationship pair trajectories: 无
- state tension: 无
- goal tension: 无
- relationship tension: 无
- conflict candidates: 无

## Suggested Order
1. 先修 `draft` 里 `gate=FAIL`、`recommendation=targeted_rewrite` 的章节，再看 `pairs / triples`
2. 再修 `chapter-plan` 与 `story-plan` 的字段错位和空字段
3. 如果某章评分里 `一致性准备度` 明显偏低，先跑 `sentinel consistency suspects` 再决定是否只是局部误写
4. 如果已经锁定某条 Story，要逐条复核一致性候选，直接跑 `sentinel consistency review-queue novel1 --story storyN`
5. 做完一轮局部复核后，立刻跑 `sentinel consistency feedback-summary novel1 --story storyN` 看这一条 Story 是否开始收敛
6. 最后补 `concept` 缺口，避免下游继续空转
