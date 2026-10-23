# Template Candidate Catalog

- novel: `novel-b`
- stories: `1`
- source: `tests/fixtures/catalog-cbk/novel-b/draft-stats`

## Learning Anchors
- 无

## Merged Families
- `像/活像模板` stories=`1` total=`2` buckets=`patterns`
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `局部疲劳窗口` stories=`1` total=`2` buckets=`fatigue_window`
  样例："线动了 | "她说 | 他抬头 | 冷线的尽头滴了一下 | 他把袖子挽起来，蹲下去
- `得像` stories=`1` total=`2` buckets=`patterns`
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `把字操作句` stories=`1` total=`2` buckets=`patterns`
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `短句密度` stories=`1` total=`2` buckets=`sentence_length`
  样例：L5 3字："线动了
- `？` stories=`1` total=`2` buckets=`punctuation`
  样例："哪头？"她问。
- `动作操作` stories=`1` total=`1` buckets=`ba_operation_context`
  样例：他把手掌贴在墙上
- `短句对白块` stories=`1` total=`1` buckets=`dialogue`
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"
- `短并列三拍` stories=`1` total=`1` buckets=`custom_template`
  样例：冷线从墙里出来，经过三扇门，停在他的桌角。
- `肯定判断/解释腔` stories=`1` total=`1` buckets=`patterns`
  样例："那是什么？"
- `连续短对白` stories=`1` total=`1` buckets=`dialogue`
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"

## Template Candidates
- `patterns::像/活像模板` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `fatigue_window::局部疲劳窗口` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例："线动了 | "她说 | 他抬头 | 冷线的尽头滴了一下 | 他把袖子挽起来，蹲下去
- `patterns::得像` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。
- `patterns::把字操作句` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `sentence_length::短句密度` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：L5 3字："线动了
- `punctuation::？` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例："哪头？"她问。
- `ba_operation_context::动作操作` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他把手掌贴在墙上
- `dialogue::短句对白块` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"
- `custom_template::短并列三拍` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：冷线从墙里出来，经过三扇门，停在他的桌角。
- `patterns::肯定判断/解释腔` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例："那是什么？"
- `dialogue::连续短对白` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例："你摸到过这个温度吗？"她问。 | "没有。" | "那是什么？" | "是线在烧。"

## Term Candidates
- `learned_filter::一下` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `learned_filter::停在` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：冷线从墙里出来，经过三扇门，停在他的桌角。
- `learned_filter::抬头` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：他抬头。冷线的尽头滴了一下。他把袖子挽起来，蹲下去。他听见墙里有水。水声从冷线的那一头传过来，很轻。
- `learned_filter::看着` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：水声重新响起来。比刚才粗。他退后一步，看着冷线的尽头。冷线在抖。抖得像一根绷紧的弦。
- `learned_filter::终端` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：风从门外进来。门外的港口灰得像一块擦不净的布。他在门口站了很久，才把终端重新拿出来。

## Keep Candidates
- `keep_candidates::场面功能有切换` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  说明：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。
- `keep_candidates::章末收束未模板化` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  说明：章末没有落入现有意象/流程词模板，说明结尾功能有继续发展的空间。
- `keep_candidates::局部排比可视作风格点缀` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  说明：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。

## Deposition Targets
- 无

## Writeback Queue
- 无

## Next Actions
1. 先看 `Merged Families` 和 `Writeback Queue`，避免同名异桶重复判断。
2. 再核 `Keep Candidates`，把设计性重复和局部节奏从纯负向规则里拆出来。
3. 最后按 `Deposition Targets` 分流到模板库、词库、评审指南或本书规则。

