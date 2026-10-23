# Review Kit

- story: `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story1`
- chapters: `2`
- scorecards: `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/scorecards/SUMMARY.md`
- learning: `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/learning/SUMMARY.md`
- profiles: `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/profiles/SUMMARY.md`
- template_backlog: `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/template-backlog/SUMMARY.md`
- template_candidates: `tests/fixtures/catalog-cbk/novel-a/draft-stats/arc1/story1/template-backlog/CANDIDATES.json`

## Current State
- gate `FAIL` x2
- recommendation `targeted_rewrite` x2
- pending_consistency_rows: `0`

## Review Assignments
- P1: 复核 `ch01-信号` 的 FAIL scorecard（source=`scorecards`）：gate=`FAIL` recommendation=`targeted_rewrite` warn_sections=`16` hard_flags=`30` 动作：先读 `scorecards/` 对应章节，再按 P1 reminders 和 hard flags 定位局部重写点。
- P1: 复核 `ch02-断桥` 的 FAIL scorecard（source=`scorecards`）：gate=`FAIL` recommendation=`targeted_rewrite` warn_sections=`14` hard_flags=`22` 动作：先读 `scorecards/` 对应章节，再按 P1 reminders 和 hard flags 定位局部重写点。
- P1: 复核 `ch01-信号`：局部句式疲劳窗口（source=`review_reminders/定位`）：同一小段里短句、判断解释、把字操作或角色起手叠加，会比单项总数更影响观感。 动作：保留一个最有用的节奏点；其余改成动作因果、环境反应、人物误读或视角入口。
- P1: 复核 `ch01-信号`：短句正在变成默认节拍（source=`review_reminders/节奏`）：连续短句会把动作、情绪和信息压成碎拍。 动作：每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。
- P1: 复核 `ch02-断桥`：局部句式疲劳窗口（source=`review_reminders/定位`）：同一小段里短句、判断解释、把字操作或角色起手叠加，会比单项总数更影响观感。 动作：保留一个最有用的节奏点；其余改成动作因果、环境反应、人物误读或视角入口。
- P1: 复核 `ch02-断桥`：短句正在变成默认节拍（source=`review_reminders/节奏`）：连续短句会把动作、情绪和信息压成碎拍。 动作：每个短句连发块只保留一个节拍点，其余改成动作因果或场面阻力。
- P1: 复核 `ch02-断桥`：线索被面板词收拢（source=`review_reminders/信息`）：归档、首屏、标签、坐标、重合等词密集时，章节会像任务列表。 动作：把一次完整结论拆成发现、排除、误判、半确认；章末改用行动阻力收束。
- P2: 复核 `ch01-信号`：对白像互答录音（source=`review_reminders/对话`）：短对白连续互顶时，场面动作会消失。 动作：保留最有锋芒的两句，其余用动作、环境声、第三方反应或设备反馈打断。
- P2: 复核 `ch01-信号`：把字句过密（source=`review_reminders/动作`）：把 X 拖上/放进/压住/推过去连续出现时，场面像操作日志。 动作：工具操作可保留必要句；线索操作拆发现-误读-后果，情绪动作改身体反应或他人误读。
- P2: 复核 `ch01-信号`：比喻模板过密（source=`review_reminders/文风`）：像/活像类句式能快速给气氛，但过密时会替代真实动作。 动作：每章保留少数最有新意的比喻，其余改成具体动作、声音、物件变化。
- P2: 复核 `ch01-信号`：章末收束可能模板化（source=`review_reminders/章末`）：章末反复用冷光、夜、首屏、继续、下一步等词，会让钩子同质。 动作：在动作余波、关系变化、外部阻力三类里换一种收束手势。
- P2: 复核 `ch01-信号`：近距离视角锚点漂移（source=`review_reminders/视角`）：同段多人物心理暴露或近距离切锚偏多时，读者会丢当前镜头中心。 动作：近距离段先固定一个感知中心，其余人物只通过动作、台词和误读出现。
- P2: 复核 `ch01-信号`：高频词/点名过密（source=`review_reminders/词汇`）：人物名、地名、设备名过密时，叙述会像点名册或设定表。 动作：先改最密的 1-2 个窗口，不要只做同义词替换。
- P2: 复核 `ch01-信号`：黏糊词/弱判断偏密（source=`review_reminders/文风`）：轻轻、微微、有点、显得等词会削弱动作力度。 动作：优先删弱判断词；用可见动作和场面反应表达轻重。

## Suggested Flow
1. 先读 `Review Assignments`，按 P1/P2 处理可指派复审任务。
2. 再读 `scorecards/SUMMARY.md`，确认这条 Story 当前是 `PASS / WATCH / FAIL` 哪一侧。
3. 再读 `learning/SUMMARY.md`，确认重复模板、沉淀目标和 plan-draft 漂移是否集中。
4. 再读 `profiles/SUMMARY.md`，确认句式骨架、人物声音和场景色调是不是同一类问题反复出现。
5. 再读 `template-backlog/SUMMARY.md`，把坏模式候选和可保留风格候选拆开看。
8. 最后再决定这轮该沉淀模板、词库、规则，还是回修正文 / chapter-plan / story-plan。

## Template Hotspots
- `patterns::把字操作句` x2
- `custom_template::重叠词节奏` x2
- `punctuation::？` x2
- `sentence_length::短句密度` x2
- `ba_operation_context::动作操作` x2
- `tracked_term::终端` x2
- `patterns::像/活像模板` x1
- `custom_template::说完就` x1

## Chapter Paths
- `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story1/ch01-信号.md`
- `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story1/ch02-断桥.md`

