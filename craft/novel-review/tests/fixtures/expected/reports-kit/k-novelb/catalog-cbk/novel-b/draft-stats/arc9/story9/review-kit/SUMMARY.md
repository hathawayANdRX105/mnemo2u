# Review Kit

- story: `tests/fixtures/catalog-cbk/novel-b/drafts/arc9/story9`
- chapters: `2`
- scorecards: `tests/fixtures/catalog-cbk/novel-b/draft-stats/arc9/story9/scorecards/SUMMARY.md`
- learning: `tests/fixtures/catalog-cbk/novel-b/draft-stats/arc9/story9/learning/SUMMARY.md`
- profiles: `tests/fixtures/catalog-cbk/novel-b/draft-stats/arc9/story9/profiles/SUMMARY.md`
- template_backlog: `tests/fixtures/catalog-cbk/novel-b/draft-stats/arc9/story9/template-backlog/SUMMARY.md`
- template_candidates: `tests/fixtures/catalog-cbk/novel-b/draft-stats/arc9/story9/template-backlog/CANDIDATES.json`

## Current State
- gate `WATCH` x2
- recommendation `light_revise` x2
- pending_consistency_rows: `0`

## Review Assignments
- P1: 复核 `ch01-冷线` 的 WATCH scorecard（source=`scorecards`）：gate=`WATCH` recommendation=`light_revise` warn_sections=`7` hard_flags=`14` 动作：先读 `scorecards/` 对应章节，再按 P1 reminders 和 hard flags 定位局部重写点。
- P1: 复核 `ch02-水声` 的 WATCH scorecard（source=`scorecards`）：gate=`WATCH` recommendation=`light_revise` warn_sections=`9` hard_flags=`15` 动作：先读 `scorecards/` 对应章节，再按 P1 reminders 和 hard flags 定位局部重写点。
- P1: 复核 `ch01-冷线`：局部句式疲劳窗口（source=`review_reminders/定位`）：同一小段里短句、判断解释、把字操作或角色起手叠加，会比单项总数更影响观感。 动作：保留一个最有用的节奏点；其余改成动作因果、环境反应、人物误读或视角入口。
- P1: 复核 `ch01-冷线`：短句正在变成默认节拍（source=`review_reminders/节奏`）：连续短句会把动作、情绪和信息压成碎拍。 动作：每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。
- P1: 复核 `ch02-水声`：局部句式疲劳窗口（source=`review_reminders/定位`）：同一小段里短句、判断解释、把字操作或角色起手叠加，会比单项总数更影响观感。 动作：保留一个最有用的节奏点；其余改成动作因果、环境反应、人物误读或视角入口。
- P1: 复核 `ch02-水声`：短句正在变成默认节拍（source=`review_reminders/节奏`）：连续短句会把动作、情绪和信息压成碎拍。 动作：每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。
- P2: 复核 `ch01-冷线-ch02-水声` 的同类章末连发（source=`ending_trends`）：连续 `2` 章落在 `意象压轴`，跨章读感可能开始同质化。 动作：对照 pairs / triples 的 `endings=` 和 `repeated=`，确认这些结尾是在推进不同后果，还是只是在重复同一种收束手势。
- P2: 复核 `ch01-冷线-ch02-水声` 的章末-色调合流（source=`trend_convergence`）：连续 `2` 章同时落在章末 `意象压轴` 和色调 `cold`，跨章读感可能开始同温度、同收束。 动作：先看 scorecards / profiles 的 tone 与 ending trend，再判断这些章节是在持续累积压迫，还是已经写成同一种章末氛围模板。
- P2: 复核 `ch01-冷线`：把字句过密（source=`review_reminders/动作`）：把 X 拖上/放进/压住/推过去连续出现时，场面像操作日志。 动作：工具操作可保留必要句；线索操作拆发现-误读-后果，情绪动作改身体反应或他人误读。
- P2: 复核 `ch01-冷线`：比喻模板过密（source=`review_reminders/文风`）：像/活像类句式能快速给气氛，但过密时会替代真实动作。 动作：每章保留少数最有新意的比喻，其余改成具体动作、声音、物件变化。
- P2: 复核 `ch01-冷线`：高频词/点名过密（source=`review_reminders/词汇`）：人物名、地名、设备名过密时，叙述会像点名册或设定表。 动作：先改最密的 1-2 个窗口，不要只做同义词替换。
- P2: 复核 `ch02-水声`：把字句过密（source=`review_reminders/动作`）：把 X 拖上/放进/压住/推过去连续出现时，场面像操作日志。 动作：工具操作可保留必要句；线索操作拆发现-误读-后果，情绪动作改身体反应或他人误读。
- P2: 判断跨章模板候选 `patterns::像/活像模板` 是否该沉淀（source=`template_backlog`）：该候选在本 Story 中出现 `2` 次，已经值得人工区分坏重复、词库项或设计性保留。 动作：读 `template-backlog/SUMMARY.md` 和 `CANDIDATES.json`，决定写回模板库、词库、规则，还是标记为 keep。
- P2: 判断跨章模板候选 `patterns::得像` 是否该沉淀（source=`template_backlog`）：该候选在本 Story 中出现 `2` 次，已经值得人工区分坏重复、词库项或设计性保留。 动作：读 `template-backlog/SUMMARY.md` 和 `CANDIDATES.json`，决定写回模板库、词库、规则，还是标记为 keep。

## Suggested Flow
1. 先读 `Review Assignments`，按 P1/P2 处理可指派复审任务。
2. 再读 `scorecards/SUMMARY.md`，确认这条 Story 当前是 `PASS / WATCH / FAIL` 哪一侧。
3. 再读 `learning/SUMMARY.md`，确认重复模板、沉淀目标和 plan-draft 漂移是否集中。
4. 再读 `profiles/SUMMARY.md`，确认句式骨架、人物声音和场景色调是不是同一类问题反复出现。
5. 再读 `template-backlog/SUMMARY.md`，把坏模式候选和可保留风格候选拆开看。
8. 最后再决定这轮该沉淀模板、词库、规则，还是回修正文 / chapter-plan / story-plan。

## Template Hotspots
- `patterns::把字操作句` x2
- `patterns::像/活像模板` x2
- `patterns::得像` x2
- `punctuation::？` x2
- `sentence_length::短句密度` x2
- `fatigue_window::局部疲劳窗口` x2
- `custom_template::短并列三拍` x1
- `tracked_term::终端` x1

## Chapter Paths
- `tests/fixtures/catalog-cbk/novel-b/drafts/arc9/story9/ch01-冷线.md`
- `tests/fixtures/catalog-cbk/novel-b/drafts/arc9/story9/ch02-水声.md`

