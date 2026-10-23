# Template Backlog

- story: `tests/fixtures/catalog-cbk/novel-a/drafts/arc1/story1`
- chapters: `2`

## Repeat Candidates
- `patterns::把字操作句` x2
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `custom_template::重叠词节奏` x2
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `punctuation::？` x2
  样例："信号在哪？"她问。
- `sentence_length::短句密度` x2
  样例：L3 4字：终端亮了
- `ba_operation_context::动作操作` x2
  样例：他把伞收起来
- `tracked_term::终端` x2
  样例：终端亮了。终端又亮了。终端还在亮。他盯着那行灰字，像盯着一口深井。
- `patterns::像/活像模板` x1
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `custom_template::说完就` x1
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `custom_template::短并列三拍` x1
  样例：他说完就不再开口。雨开始落下来，落在桥面上，落在轨道上，落在他没合拢的领口里。他把伞收起来。他把外套披在她肩上。他把那只旧终端揣进怀里。
- `tokens::然后` x1
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `tracked_term::屏幕` x1
  样例：第七区的灯一排一排暗下去。屏幕上的灰字重新跳出来。他继续排查。他新建了一组日志。桥在远处亮着蓝光。夜色像一块压下来的灰布，铺满整个港口。
- `tracked_term::抬头` x1
  样例：然后他走。然后他停。然后他又走。接着他抬头。接着他压低声音。
- `patterns::线索面板词` x1
  样例：夜落在港口上。灯落在桥上。冷光落在他的肩上。他继续排查。新分组已建立。首屏的灰字还没有变。
- `patterns::得像` x1
  样例：他把手贴在桥面上。桥面是凉的，凉得像一整条河。他慢慢后退。他慢慢蹲下。他慢慢把耳朵贴上去。
- `fatigue_window::局部疲劳窗口` x1
  样例："她问 | "铁 | "什么样的铁 | "旧的铁 | 他把手贴在桥面上
- `tracked_term::冷光` x1
  样例：夜落在港口上。灯落在桥上。冷光落在他的肩上。他继续排查。新分组已建立。首屏的灰字还没有变。

## Keep Candidates
- `局部排比可视作风格点缀` x2：检测到少量 AA/BB 或短分句节奏，但还没形成模板疲劳，可以先按风格候选保留。
- `场面功能有切换` x2：粗分块没有被单一对白或说明吃满，说明这一章至少在尝试做功能接力。

## Deposition Targets
- `novel1/rules/draft.md` x2
- `skills/review-guide.md` x1
- `configs/rules/review.yaml#draft.tracked_terms` x1

## Next Actions
1. 先看 `Repeat Candidates` 里跨章反复出现的家族，判断它该进模板库、词库，还是只算局部问题。
2. 再看 `Keep Candidates`，避免把本来应保留的节奏、章末收束或动作后果误杀。
3. 最后按 `Deposition Targets` 决定写回 `configs/rules/review.yaml`（`draft.template_rules` / `draft.tracked_terms`）、`skills/review-guide.md` 还是本书规则。

