# story1-ch01 Review Learning Log

- source: `tests/fixtures/consistency/novel1/drafts/story1-ch01.md`
- usage: 这不是最终审查结论，而是为‘确认 / 驳回 / 沉淀’准备的学习清单。

## Manual Decisions
- confirmed_issues: `_fill_in_`
- false_positives: `_fill_in_`
- design_repeats_to_keep: `_fill_in_`
- missing_checks: `_fill_in_`

## Template Backlog
- `[pending]` `sentence_length` `短句密度`：短句过多或连发，会让草稿像节拍器或对白录音

## Bonus Candidates
- `[pending]` `章末收束`：本章章末没有命中模板警告，可人工确认它是不是值得保留的收束方式。
  样例：# 第一幕

信号枪裂开了，接口失灵。陆沉稳住伤势，血止住了。
阿沉擦伤缠住，但还能打。
陆沉提防阿沉，怀疑他泄密。
任务交给陆沉，目标变成先走。

## Rule / Bank Suggestions
- `[pending]` `novel1/research/consistency/review-feedback.jsonl`：当前 story 还有 3 条中高置信度一致性候选未复核，先补反馈再决定是否继续扩规则。
- `[pending]` `novel1/research/consistency/review-feedback.jsonl`：`story1` 仍有 1 条长期待观察反馈，说明这条 Story 的一致性口径还没真正收敛。

## Consistency Feedback Snapshot
- story: `story1`
- feedback_log: `tests/fixtures/consistency/novel1/research/consistency/review-feedback.jsonl`
- confirmed=`0`
- false_positive=`0`
- designed_keep=`0`
- watch=`1`
- pending=`3`
- review_queue: `sentinel consistency review-queue novel1 --story story1`
- feedback_summary: `sentinel consistency feedback-summary novel1 --story story1`
- facets:
  - `state_progression` x1
- pending rows:
  - `goal_state_drift` `陆沉` confidence=`high` assigned=交给,任务 ; changed=目标变成 ; completed=收尾
  - `relationship_tone_shift` `陆沉` confidence=`high` close=接住,接应 ; distant=提防,怀疑
  - `injury_state_jump` `陆沉` confidence=`medium` injury=擦伤 -> 稳住,止住,还能打
- pending actions:
  - `goal_state_drift` `陆沉` confidence=`high`：先回看 fact cue 和上下文段，判断这是真跳变还是阶段推进。
  - command: `sentinel consistency feedback-add novel1 --category goal_state_drift --story story1 --title '陆沉' --decision watch --summary-contains 'assigned=交给,任务'`
  - `relationship_tone_shift` `陆沉` confidence=`high`：先回看 fact cue 和上下文段，判断这是真跳变还是阶段推进。
  - command: `sentinel consistency feedback-add novel1 --category relationship_tone_shift --story story1 --title '陆沉' --decision watch --summary-contains 'close=接住,接应'`
  - `injury_state_jump` `陆沉` confidence=`medium`：先回看 fact cue 和上下文段，判断这是真跳变还是阶段推进。
  - command: `sentinel consistency feedback-add novel1 --category injury_state_jump --story story1 --title '陆沉' --decision watch --summary-contains 'injury=擦伤'`

## Plan Alignment Review
- 无 plan-draft 对齐快照

## Ending Signal Review
- ending_signal=`未归类`
- 当前章末未触发模板化警告；复盘时优先判断它是否在承担新的后果，而不是只因为不重复就直接加分。

## Feedback-Derived Backlog
- `novel1/research/consistency/review-feedback.jsonl`：`story1` 仍有 1 条长期待观察反馈，说明这条 Story 的一致性口径还没真正收敛。

## Reminder Snapshot
- `P2` `节奏` 短句正在变成默认节拍：连续短句会把动作、情绪和信息压成碎拍。

## Next Review Questions
- 这些重复里，哪些其实承担了人物声音、压迫感、节奏或讽刺功能？
- 这章真正要沉淀的是模板、词库、规则，还是只是一个局部问题？
- 如果这类问题再次出现，下一轮脚本应该如何更早抓到它？

