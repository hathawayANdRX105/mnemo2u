# Template Candidate Catalog

- novel: `novel1`
- stories: `1`
- source: `tests/fixtures/consistency/novel1/draft-stats`

## Learning Anchors
- 无

## Merged Families
- `短句密度` stories=`1` total=`2` buckets=`sentence_length`
  样例：L4 6字：任务结束，交差

## Template Candidates
- `sentence_length::短句密度` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  样例：L4 6字：任务结束，交差

## Term Candidates
- 无

## Keep Candidates
- `keep_candidates::章末收束未模板化` stories=`1` total=`2` target=`configs/rules/review.yaml#draft.template_rules`
  说明：章末没有落入现有意象/流程词模板，说明结尾功能有继续发展的空间。

## Deposition Targets
- 无

## Writeback Queue
- 无

## Next Actions
1. 先看 `Merged Families` 和 `Writeback Queue`，避免同名异桶重复判断。
2. 再核 `Keep Candidates`，把设计性重复和局部节奏从纯负向规则里拆出来。
3. 最后按 `Deposition Targets` 分流到模板库、词库、评审指南或本书规则。

