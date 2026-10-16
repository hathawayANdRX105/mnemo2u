# Model profiles — device-tier model calling interface (spec)

**Status**: spec (S0). Implementation lands on this branch in later commits; the PR stays draft until then.
**Local research companion**: `todo/design/06-model-profiles.md` (gitignored working notes).

## Goal

One capability abstraction over every model call, so that **phone/desktop/cloud run the same RAG with
different model stacks by changing a profile string** — callers never name a model.

Existing seams this builds on (do not re-invent):

| Piece | Coordinate | Note |
|---|---|---|
| Capability traits | `Embedder` / `LlmClient` / `JudgeClient` (`src/core/traits.rs`, `src/llm/judge.rs`) | narrow interfaces, distinct return semantics |
| Null objects | `DisabledJudge`, `MockEmbedder`, `MockLlm` | explicit degradation, not scattered `None` |
| Wrappers | `CachedLlm` / `CachedJudge` (cache key includes model) | cross-cutting concerns live in wrappers |
| Promise | R4 doc §抽象方案: "swap judge backend = add one adapter, zero changes elsewhere" | this spec writes the promise out fully |

## Interface (sketch)

```rust
// src/llm/profile.rs
pub struct CapabilitySpec {           // static description of one implementation
    pub name: &'static str,           // "potion-base-8M" / "jev-nano" / "systemone:http"
    pub capability: Capability,       // Embed | Judge | Rerank
    pub langs: &'static [&'static str],
    pub disk_mb: u32,                 // declared footprint, used by the budget guard
    pub ram_mb: u32,
    pub context_tokens: u32,          // jev-nano=384, SystemOne default=32768
    pub calibrated: bool,             // only implementations with ECE/conformal evidence
}

pub struct ModelProfile {             // one device tier = one impl per capability + budgets
    pub name: String,                 // "phone-lite" / "phone-plus" / "desktop-local" / "cloud"
    pub max_ram_mb: u32,
    pub max_disk_mb: u32,
    pub embed: String,
    pub judge: String,
    pub rerank: Option<String>,
}

pub fn assemble(profile: &ModelProfile, deps: &Deps) -> Result<ModelStack, ProfileError>;
// ModelStack { embedder: Arc<dyn Embedder>, judge: Arc<dyn JudgeClient>, reranker: Option<Arc<dyn Reranker>> }
```

## Assembly-time disciplines (fail fast, all three in `assemble`)

1. **Budget guard** — sum declared `disk_mb`/`ram_mb` over the profile's picks; over budget ⇒ refuse
   assembly and name the offending component(s). A combination that cannot fit on the device must fail
   at assembly, never silently at runtime.
2. **Language guard** — profiles declare `langs`; the session language selects the embedding/NLI
   implementation (e.g. English `potion` vs Chinese `bge-small-zh`); a cross-language pick is refused
   instead of silently degrading.
3. **Calibration guard** — `calibrated: false` implementations may not be used with hard thresholds
   (warn-branch only). The threshold discipline from the mobile research becomes a startup check.

## Unified SystemOne adapter (the key subtraction)

`jev`, `jev-nano`, `KaLM-Jev`, `OpenDecider(serve)` and `laya[serve]` all speak the same shape:
`POST /v1/systemone` with `state + questions` (`noul` / `choice` / `score`) returning probability
distributions. Therefore:

- **One** `SystemOneHttpAdapter { base_url, model_id, spec: &CapabilitySpec }` covers all of them;
  differences (context limit, supported question types, abstention semantics) ride the descriptor,
  not code branches.
- In-process forms (blink C99/wasm, ORT-mobile, `laya[onnx]`) are separate adapter types behind the
  same trait. First version may implement HTTP only (a local sidecar is still local).
- Abstention is first-class: a low-confidence SystemOne answer surfaces as a typed judge error and
  feeds the fallback chain below.

## Degradation chains (fixed at assembly)

```
judge  : systemone:http(remote) -> systemone:http(local nano) -> heads(in-proc) -> Disabled
embed  : onnx-small(zh/en)      -> static-8M                  -> static-2M
rerank : minilm-L2              -> tinybert                   -> None (keep fusion order)
```

Every downgrade emits `degraded: [capability:from->to]` (extension of R3 contract 2); recall results
stay observable.

## Device profile table

| Profile | embed | compare (NLI) | judge | rerank | disk total |
|---|---|---|---|---|---|
| phone-lite (≤100MB) | potion-2M/8M or bge-small-zh int8 | bert-tiny-mnli int8 (EN) | jev-nano 8MB or blink 66KB | TinyBERT int8 | ~25-40MB |
| phone-plus (≤200MB) | bge-small-zh int8 (~24MB) or potion-8M | + multilingual-MiniLMv2-L6-mnli-xnli int8 [size TBC, ~100MB] | same as lite | MiniLM-L2 int8 (~23MB) | ~60-150MB |
| desktop-local (≤2GB) | bge-small / e5-small int8 | nli-MiniLM2 / mDeBERTa-xnli ONNX (1.1GB) | laya single ckpt (1.3GB) / OpenDecider-nano (0.8GB) | bge-reranker-base int8 | 1.5-2.5GB |
| cloud | — | — | jev endpoint | — | 0 |

Memory-ladder rationale and the benefit-verification protocol:
[`docs/model-memory-ladder.md`](model-memory-ladder.md).

## Acceptance criteria (implemented in later commits on this branch)

- A1 budget guard unit test: an over-budget profile is refused with the offending components named.
- A2 swap test: changing the profile changes the assembled stack with zero diff in write/read
  wiring (extends R4 AC6 to profile granularity).
- A3 degradation test: each downgrade step is observable via `degraded` tags.
- A4 SystemOne contract smoke: wire-shape requests against a mock/local server, 19/19-style checks.
- A5 ladder evaluation: frozen criteria + per-tier accuracy/ECE/abstain/latency/RSS report.

## Tasks (this PR)

- [x] S0 spec: this file + `docs/model-memory-ladder.md`
- [ ] S1 `CapabilitySpec` registry + `assemble()` + budget guard (A1)
- [ ] S2 `SystemOneHttpAdapter` (base_url/model/spec) + wire smoke (A4)
- [ ] S3 degradation chains + `degraded` tags (A3)
- [ ] S4 `[profiles.*]` config section (depends on the toml loader from PR #9) + swap proof (A2)
- [ ] S5 ladder evaluation protocol implementation (A5)

## Open items

- multilingual-MiniLMv2-L6-mnli-xnli int8 exact size and Chinese XNLI quality — TBC.
- blink: whether an HTTP wrapper exists; if C-API only, thin FFI or local shim.
- Paired-desktop judge over LAN security boundary (loopback/LAN only, no credential persistence).
