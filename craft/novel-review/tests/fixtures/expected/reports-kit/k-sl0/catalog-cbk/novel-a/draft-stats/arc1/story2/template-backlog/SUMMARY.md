# Template Backlog

- story: `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story2`
- chapters: `2`

## Repeat Candidates
- `patterns::线索面板词` x2
- `patterns::把字操作句` x2
- `punctuation::？` x2
- `sentence_length::短句密度` x2
  样例：L3 5字：他走进灰港
- `fatigue_window::局部疲劳窗口` x2
  样例：他新建了一组日志 | 他把旧日志归档 | 他把第三屏拉出来 | "为什么是这里 | "她问
- `tracked_term::终端` x2
- `tracked_term::一下` x2
- `dialogue_axis_gap::对白转轴缺口` x2
  样例："它在数 | "她说 | "数什么 | "数我们
- `patterns::像/活像模板` x1
- `patterns::得像` x1
- `tokens::因为` x1
- `ba_operation_context::动作操作` x1
- `custom_template::短并列三拍` x1
- `tracked_term::屏幕` x1
- `ending::章末模板` x1
  样例：么在数。一步。两步。停。三步。  "数完了会怎样？"她问。  "不知道。"他把终端揣回怀里，"但第七区的灯会亮。"  第七区没有亮。灰字还跳。他新建了一组日志。

## Keep Candidates
- `场面功能有切换` x2：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。
- `动作段有后果反馈` x1：冲突段不只累计动作动词，也带出了结果、伤害或位移反馈，可以视作紧凑度候选。
- `局部排比可视作风格点缀` x1：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。

## Deposition Targets
- 无

## Next Actions
1. 先看 `Repeat Candidates` 里跨章反复出现的家族，判断它该进模板库、词库，还是只算局部问题。
2. 再看 `Keep Candidates`，避免把本来应保留的节奏、章末收束或动作后果误杀。
3. 最后按 `Deposition Targets` 决定写回 `configs/rules/review.yaml`（`draft.template_rules` / `draft.tracked_terms`）、`skills/review-guide.md` 还是本书规则。

