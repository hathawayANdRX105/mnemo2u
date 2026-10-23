# Template Backlog

- story: `tests/fixtures/catalog-cbk/novel-b/drafts/arc9/story9`
- chapters: `2`

## Repeat Candidates
- `patterns::把字操作句` x2
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `patterns::像/活像模板` x2
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `patterns::得像` x2
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `punctuation::？` x2
  样例："哪头？"她问。
- `sentence_length::短句密度` x2
  样例：L5 3字："线动了
- `fatigue_window::局部疲劳窗口` x2
  样例："线动了 | "她说 | 他抬头 | 冷线的尽头滴了一下 | 他把袖子挽起来，蹲下去
- `custom_template::短并列三拍` x1
  样例：冷线从墙里出来，经过三扇门，停在他的桌角。
- `tracked_term::终端` x1
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `tracked_term::抬头` x1
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `tracked_term::停在` x1
  样例：冷线从墙里出来，经过三扇门，停在他的桌角。
- `tracked_term::一下` x1
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `patterns::肯定判断/解释腔` x1
  样例："那是什么？"
- `ba_operation_context::动作操作` x1
  样例：他把手掌贴在墙上
- `tracked_term::看着` x1
  样例：水声重新响起来。比刚才粗。他退后一步，看着冷线的尽头。冷线在抖。抖得像一根绷紧的弦。
- `dialogue::连续短对白` x1
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"
- `dialogue::短句对白块` x1
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"

## Keep Candidates
- `章末收束未模板化` x2：章末没有落入现有意象/流程词模板，说明结尾功能有继续发展的空间。
- `场面功能有切换` x2：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。
- `局部排比可视作风格点缀` x1：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。

## Deposition Targets
- 无

## Next Actions
1. 先看 `Repeat Candidates` 里跨章反复出现的家族，判断它该进模板库、词库，还是只算局部问题。
2. 再看 `Keep Candidates`，避免把本来应保留的节奏、章末收束或动作后果误杀。
3. 最后按 `Deposition Targets` 决定写回 `configs/rules/review.yaml`（`draft.template_rules` / `draft.tracked_terms`）、`skills/review-guide.md` 还是本书规则。

