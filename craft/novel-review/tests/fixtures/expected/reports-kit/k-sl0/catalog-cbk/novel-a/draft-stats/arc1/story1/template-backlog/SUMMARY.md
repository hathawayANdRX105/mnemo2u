# Template Backlog

- story: `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story1`
- chapters: `2`

## Repeat Candidates
- `patterns::把字操作句` x2
- `custom_template::重叠词节奏` x2
- `punctuation::？` x2
- `sentence_length::短句密度` x2
  样例：L3 4字：终端亮了
- `ba_operation_context::动作操作` x2
- `tracked_term::终端` x2
- `patterns::像/活像模板` x1
- `custom_template::说完就` x1
- `custom_template::短并列三拍` x1
- `tokens::然后` x1
- `tracked_term::屏幕` x1
- `tracked_term::抬头` x1
- `patterns::线索面板词` x1
- `patterns::得像` x1
- `fatigue_window::局部疲劳窗口` x1
  样例："她问 | "铁 | "什么样的铁 | "旧的铁 | 他把手贴在桥面上
- `tracked_term::冷光` x1

## Keep Candidates
- `局部排比可视作风格点缀` x2：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。
- `场面功能有切换` x2：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。

## Deposition Targets
- `novel1/rules/draft.md` x2
- `configs/rules/review.yaml#draft.tracked_terms` x1

## Next Actions
1. 先看 `Repeat Candidates` 里跨章反复出现的家族，判断它该进模板库、词库，还是只算局部问题。
2. 再看 `Keep Candidates`，避免把本来应保留的节奏、章末收束或动作后果误杀。
3. 最后按 `Deposition Targets` 决定写回 `configs/rules/review.yaml`（`draft.template_rules` / `draft.tracked_terms`）、`skills/review-guide.md` 还是本书规则。

