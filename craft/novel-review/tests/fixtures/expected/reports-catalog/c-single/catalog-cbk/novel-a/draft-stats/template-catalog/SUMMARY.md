# Template Candidate Catalog

- novel: `novel-a`
- stories: `1`
- source: `tests/fixtures/catalog-cbk/novel-a/draft-stats`

## Learning Anchors
- 无

## Merged Families
- `像/活像模板` stories=`1` total=`1` buckets=`patterns`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `动作操作` stories=`1` total=`1` buckets=`ba_operation_context`
  样例：他把伞收起来
- `把字操作句` stories=`1` total=`1` buckets=`patterns`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `然后` stories=`1` total=`1` buckets=`tokens`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `短句密度` stories=`1` total=`1` buckets=`sentence_length`
  样例：L3 4字：终端亮了
- `短并列三拍` stories=`1` total=`1` buckets=`custom_template`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `说完就` stories=`1` total=`1` buckets=`custom_template`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `重叠词节奏` stories=`1` total=`1` buckets=`custom_template`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `？` stories=`1` total=`1` buckets=`punctuation`
  样例："信号在哪？"她问。

## Template Candidates
- `patterns::像/活像模板` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `ba_operation_context::动作操作` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他把伞收起来
- `patterns::把字操作句` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `tokens::然后` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `sentence_length::短句密度` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：L3 4字：终端亮了
- `custom_template::短并列三拍` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `custom_template::说完就` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `custom_template::重叠词节奏` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `punctuation::？` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  样例："信号在哪？"她问。

## Term Candidates
- `learned_filter::屏幕` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `learned_filter::抬头` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `learned_filter::终端` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.tracked_terms`
  样例：终端亮了。终端又亮了。终端还在亮。他盯着那行灰字，像盯着一口深井。

## Keep Candidates
- `keep_candidates::场面功能有切换` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  说明：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。
- `keep_candidates::局部排比可视作风格点缀` stories=`1` total=`1` target=`configs/rules/review.yaml#draft.template_rules`
  说明：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。

## Deposition Targets
- `skills/review-guide.md` x1
- `novel1/rules/draft.md` x1

## Writeback Queue
- 无

## Next Actions
1. 先看 `Merged Families` 和 `Writeback Queue`，避免同名异桶重复判断。
2. 再核 `Keep Candidates`，把设计性重复和局部节奏从纯负向规则里拆出来。
3. 最后按 `Deposition Targets` 分流到模板库、词库、评审指南或本书规则。

