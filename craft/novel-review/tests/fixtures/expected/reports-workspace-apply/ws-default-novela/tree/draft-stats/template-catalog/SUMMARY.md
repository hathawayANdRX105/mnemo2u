# Template Candidate Catalog

- novel: `novel-a`
- stories: `2`
- source: `tests/fixtures/catalog-cbk/novel-a/draft-stats`

## Learning Anchors
- `keep` `keep_candidates::场面功能有切换` stories=`2` total=`4` -> `configs/rules/review.yaml#draft.template_rules`
  说明：多条 Story 都在把它当加分或保留候选，说明审查不该一刀切地误杀这类文笔设计。
- `keep` `keep_candidates::局部排比可视作风格点缀` stories=`2` total=`3` -> `configs/rules/review.yaml#draft.template_rules`
  说明：多条 Story 都在把它当加分或保留候选，说明审查不该一刀切地误杀这类文笔设计。

## Merged Families
- `把字操作句` stories=`2` total=`4` buckets=`patterns`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `短句密度` stories=`2` total=`4` buckets=`sentence_length`
  样例：L3 4字：终端亮了
- `？` stories=`2` total=`4` buckets=`punctuation`
  样例："信号在哪？"她问。
- `动作操作` stories=`2` total=`3` buckets=`ba_operation_context`
  样例：他把伞收起来
- `局部疲劳窗口` stories=`2` total=`3` buckets=`fatigue_window`
  样例："她问 | "铁 | "什么样的铁 | "旧的铁 | 他把手贴在桥面上
- `线索面板词` stories=`2` total=`3` buckets=`patterns`
  样例：夜落在港口上。灯落在桥上。冷光落在他的肩上。他继续排查。新分组已建立。首屏的灰字还没有变。
- `像/活像模板` stories=`2` total=`2` buckets=`patterns`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `得像` stories=`2` total=`2` buckets=`patterns`
  样例：他把手贴在桥面上。桥面是凉的，凉得像一整条河。他慢慢后退。他慢慢蹲下。他慢慢把耳朵贴上去。
- `短并列三拍` stories=`2` total=`2` buckets=`custom_template`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `对白转轴缺口` stories=`1` total=`2` buckets=`dialogue_axis_gap`
  样例："它在数 | "她说 | "数什么 | "数我们
- `重叠词节奏` stories=`1` total=`2` buckets=`custom_template`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `因为` stories=`1` total=`1` buckets=`tokens`
  样例："因为桥在这里。"
- `然后` stories=`1` total=`1` buckets=`tokens`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `章末模板` stories=`1` total=`1` buckets=`ending`
  样例：么在数。一步。两步。停。三步。  "数完了会怎样？"她问。  "不知道。"他把终端揣回怀里，"但第七区的灯会亮。"  第七区没有亮。灰字还跳。他新建了一组日志。
- `说完就` stories=`1` total=`1` buckets=`custom_template`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。

## Template Candidates
- `patterns::把字操作句` stories=`2` total=`4` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `sentence_length::短句密度` stories=`2` total=`4` target=`configs/rules/review.yaml#draft.template_rules`
  样例：L3 4字：终端亮了
- `punctuation::？` stories=`2` total=`4` target=`configs/rules/review.yaml#draft.template_rules`
  样例："信号在哪？"她问。
- `ba_operation_context::动作操作` stories=`2` total=`3` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他把伞收起来
- `fatigue_window::局部疲劳窗口` stories=`2` total=`3` target=`configs/rules/review.yaml#draft.template_rules`
  样例："她问 | "铁 | "什么样的铁 | "旧的铁 | 他把手贴在桥面上
- `patterns::线索面板词` stories=`2` total=`3` target=`configs/rules/review.yaml#draft.template_rules`
  样例：夜落在港口上。灯落在桥上。冷光落在他的肩上。他继续排查。新分组已建立。首屏的灰字还没有变。
- `patterns::像/活像模板` stories=`2` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `patterns::得像` stories=`2` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他把手贴在桥面上。桥面是凉的，凉得像一整条河。他慢慢后退。他慢慢蹲下。他慢慢把耳朵贴上去。
- `custom_template::短并列三拍` stories=`2` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `dialogue_axis_gap::对白转轴缺口` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例："它在数 | "她说 | "数什么 | "数我们
- `custom_template::重叠词节奏` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `tokens::因为` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例："因为桥在这里。"
- `tokens::然后` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `ending::章末模板` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：么在数。一步。两步。停。三步。  "数完了会怎样？"她问。  "不知道。"他把终端揣回怀里，"但第七区的灯会亮。"  第七区没有亮。灰字还跳。他新建了一组日志。
- `custom_template::说完就` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。

## Term Candidates
- `learned_filter::终端` stories=`2` total=`4` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：终端亮了。终端又亮了。终端还在亮。他盯着那行灰字，像盯着一口深井。
- `learned_filter::屏幕` stories=`2` total=`2` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `learned_filter::一下` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：他不知道她在说什么。但他觉得她说得对。他把终端举到雾里。灰字在雾里跳了一下。第三屏亮了。第四屏也亮了。
- `learned_filter::冷光` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：夜落在港口上。灯落在桥上。冷光落在他的肩上。他继续排查。新分组已建立。首屏的灰字还没有变。
- `learned_filter::抬头` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。

## Keep Candidates
- `keep_candidates::场面功能有切换` stories=`2` total=`4` target=`configs/rules/review.yaml#draft.template_rules`
  说明：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。
- `keep_candidates::局部排比可视作风格点缀` stories=`2` total=`3` target=`configs/rules/review.yaml#draft.template_rules`
  说明：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。
- `keep_candidates::动作段有后果反馈` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  说明：冲突段不只累计动作动词，也带出了结果、伤害或位移反馈，可以视作紧凑度候选。

## Deposition Targets
- `novel1/rules/draft.md` x2
- `skills/review-guide.md` x2
- `configs/rules/review.yaml#draft.tracked_terms` x1

## Writeback Queue
- 无

## Next Actions
1. 先看 `Merged Families` 和 `Writeback Queue`，避免同名异桶重复判断。
2. 再核 `Keep Candidates`，把设计性重复和局部节奏从纯负向规则里拆出来。
3. 最后按 `Deposition Targets` 分流到模板库、词库、评审指南或本书规则。

