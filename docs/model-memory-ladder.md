# Model memory ladder — tier combinations and benefit hypotheses

**Status**: hypothesis. Nothing here is measured against our workloads yet; the point of this document
is to freeze the combinations and the verification protocol, so that when the profile layer
(`docs/model-profiles.md`) is implemented we can run the ladder and pick per device.

**Benefit definition**: marginal gain per MB — each tier is compared against the previous one on the
same frozen evaluation set, per operation, reporting accuracy / ECE / abstain-rate / p50-p95 latency /
peak RSS. A tier earns its keep only if the Δ produces a visible, repeatable difference.

## Tiers

### T0 — ≤100MB RAM / ≤100MB disk (phone-lite)

| Component | Pick | Footprint |
|---|---|---|
| embed | potion-base-2M/8M (static, 256d) | 2-8MB int8 |
| compare | prajjwal1/bert-tiny-mnli int8 (EN) | ~5MB |
| rerank | ms-marco-TinyBERT-L-2 int8 | ~5MB |
| judge | javimosch/jev-nano (7M, 8MB tarball) or blink (66KB WASM + model file) | ~8MB |
| rules/heads | `ChunkValueScorer`, time windows, tombstone digest | ~5MB |
| data | vectors 20k-100k × 256d int8 + text/SQLite | 20-55MB |

Buys: recall, dedup, EN gate-level compare, routing, conformal abstention, all R5 symbolic ops.
Cannot buy: Chinese compare, subtle contradiction, multi-way `choice`/`score` (7M raw logprob ≈ chance —
jev-nano's own published number: 28% on JevBench easy+original).

### T1 — ≤200MB (phone-plus)

Adds **one** ~100MB-class component (the multilingual vocab alone eats ~96MB, so only one):

| Option | Adds | Hypothesis to verify |
|---|---|---|
| A: `bge-small-zh-v1.5` int8 (~24MB) | proper Chinese recall | zh recall@k jumps from unusable to bge-small-class |
| B: `multilingual-MiniLMv2-L6-mnli-xnli` int8 (size TBC) | Chinese compare | zh contradiction detection reaches "gate-grade" (~0.6-0.7 XNLI class) |
| C: MiniLM-L2 reranker int8 (~23MB) | ranking quality | rerank Δ on frozen set |

A and C together still fit (~50MB + data); B is either/or with them.

### T2 — ≤400MB (phone-max)

Adds a **real decision model**: `tasksource-jev-nano-v0` (149M; int8 ≈ 150MB) — genuine
`choice`/`score` decisions at small scale. Alternative: OpenDecider-nano int8 (~400MB) occupies the
whole tier — take it only if typed-decision quality outweighs everything else.

Hypothesis: multi-way decisions move from "near chance" (T0/T1 raw heads) to a usable band
(149M typed-decisions class ≈ 0.7 range; OpenDecider-nano measured 0.796 typed-decisions on its own
harness, ECE 0.092 — the best calibration in the open set).

### T3 — ≤800MB (desktop-nano)

`OpenDecider-nano` q8 (~450MB) + `nli-MiniLM2-L6` int8 + `bge-small` + `bge-reranker-base` int8
(≈600-700MB total with data). Buys near-desktop decision quality with browser/desktop runtimes
(ONNX; the same model already ships as a Chrome extension). Cannot buy: multilingual big NLI
(mDeBERTa-xnli ONNX is 1.1GB) or laya (single checkpoint 1.3GB).

### T4 — ≤1.5GB (desktop-full)

Pick **one** heavy component (they do not coexist within the budget):

| Option | Footprint | Buys |
|---|---|---|
| laya single checkpoint | ~1.3GB | multilingual typed decisions (MASSIVE 0.657 vs EN ckpt 0.783 — one-model tradeoff) |
| mDeBERTa-v3-base-xnli ONNX | ~1.1GB | best multilingual NLI (zh compare at quality) |
| browser-laya (WebGPU) + T3 stack | 0 disk | laya decisions in a tab, T3 for the rest |

Out of budget at every tier: jev-local instruct models (6-18GB), 3-checkpoint laya router (3.9GB),
OpenDecider-small (4B), Foq (8B), jevalt (4B/3GB).

## Marginal-benefit hypothesis table (all rows to be measured)

| Step | Expected Δ | Anchor | Risk |
|---|---|---|---|
| T0 → T1(A) | zh recall: unusable → usable | bge-small-zh is a mainstream zh embedder | vocab/tokenizer mismatch work |
| T0 → T1(B) | zh compare: n/a → gate-grade | multilingual MiniLMv2 class | int8 size may exceed 100MB headroom |
| T1 → T2 | multi-way decisions: chance → usable | 149M typed-decisions ≈ 0.7 | 149M int8 ≈ 150MB is a hard floor; RSS overhead |
| T2 → T3 | decisions + calibration: usable → strong | 0.796 acc / ECE 0.092 (OpenDecider harness) | ONNX runtime on phone still unproven |
| T3 → T4 | multilingual quality: EN-only → multilingual | laya multilingual / mDeBERTa-xnli | 1.1-1.3GB excludes everything else |

## Verification protocol (frozen before running, R6.AC5 style)

1. **Frozen criteria**: per-operation judgment sets (noul / choice / score), EN + ZH, built from the
   `laya-canon-judgments` rows format (state + typed questions + gold distribution). Criteria and
   thresholds are frozen before any tier runs; running does not change them.
2. **Per tier, per operation**: accuracy, ECE, option-order flip rate, abstain rate;
   latency p50/p95; peak RSS (`/usr/bin/time -v` or `getrusage` on the harness process).
3. **Report**: marginal-gain-per-MB curve per step; the pick per device tier is read off the curve,
   not asserted in advance.
4. **Discipline**: a tier that shows no visible Δ over its predecessor is dropped from its profile —
   the ladder is allowed to shrink.

## Open items

- Exact int8 footprints marked TBC (multilingual NLI, potion family, blink model file).
- The zh compare question may resolve via the laya-canon head route (train a zh compare head on the
  7M trunk) instead of a separate NLI model — compare both when T2 lands.
- Peak-RSS measurement tooling must be identical across tiers or the MB axis is not comparable.
