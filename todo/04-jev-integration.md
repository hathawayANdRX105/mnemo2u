# R4 Judge 全链（判断接入与外发门）

**状态**：待实施（抽象方案已定稿：三层结构，jev 仅为首个后端——见 §抽象方案；接口契约与测试矩阵见 §接口契约 / §测试矩阵）。
**目标**：把判断能力（judge：classify/compare/extract/rerank/find）接入写读两侧，并落地统一执行外壳与外发门、成本控制。**接口面向能力，不绑死 jev**——`JudgeClient` 是能力 trait，jev-compatible endpoint 只是首个后端适配器；后续换判断后端 = 新增一个 adapter，写读接线、外发门、计量、缓存零改动。无判断后端时全库可用、显式降级。
**前置**：[R2](02-write-path.md)、[R3](03-retrieval.md)验收。（均已达成：R2 见 02 接续记录 PR #1–#3；R3 见 03 接续记录 PR #4/#5，2026-10-12——读路径 `RecallOutcome.candidates` 已按 250 上限备好精排入口，本阶段可开工。）
**下一阶段**：[05-reflect](05-reflect.md)。

## 抽象方案（三层，已定稿）

**决策**：统一的是「执行外壳」，不是「模型接口」。三条能力 trait 保持窄接口、互不合并（embed 出向量 / chat 出文本 / judge 出判定，返回语义完全不同）；外壳承载一切横切关注点；每个能力一个后端 + 一个显式空对象 + 一个 mock。

```
层 1  能力 trait（窄接口，各带 DTO）
      Embedder      core/traits.rs   （已有；fastembed 本地 ONNX / MockEmbedder）
      LlmClient     llm/mod.rs       （已有；chat：抽取/报告/摘要）
      JudgeClient   llm/judge.rs     （新；classify/compare/extract/rerank/find 五操作）
层 2  执行外壳（统一入口）llm/envelope.rs（新）
      重试（408/409/429/5xx、指数退避）· 超时 · 脱敏错误分类（不泄上游体）
      · 计量行（实际模型/usage/候选规模）· 外发门 hook · 缓存机制（hash + KvStore）
      三个能力共用同一外壳；LlmClient 现路径先迁入（行为不回归）
层 3  后端适配
      Embedder:   fastembed（本地，不出网、不过门）/ MockEmbedder
      LlmClient:  OpenAI-compatible / MockLlm
      JudgeClient: JudgeEndpointClient（jev-compatible endpoint）/ DisabledJudge（默认，零调用）
                   / JudgeMock（fixture 脚本化，故障注入同形）
```

**依据**（todo/refs/ 调查，2026-10-12；SuperMew 用 codegraph 全量索引后细读，其余项目 scout 抽查）：

- **SuperMew** `backend/providers/core.py`：`ProviderExecutor` 统一执行所有外部调用；`ProviderCode` 按操作族（EMBEDDING_*/RERANK_*/MODEL_*）给稳定错误码，`classify_provider_exception` 把异常归一为脱敏 `ProviderError`。`providers/rerank.py`：`RerankerProvider` Protocol（`rerank -> RerankResult{items, attempts}`，结果与 provider 无关）+ `DisabledRerankerAdapter` 显式空对象（未配置 ≠ None 分支散落）。`model_control/contracts.py`：模型按角色（answer/fast/grader/evaluator）分配、Run 冻结快照——mnemo2u 是库不建控制面，仅固定模型 id 进缓存键即可。
- **microsoft/graphrag** `packages/graphrag-llm`：外壳是**可组合 middleware 管线**（with_retries + with_rate_limiting + with_cache + with_metrics + with_logging），在构造时注入 `LLMCompletion` 与 `LLMEmbedding` 两个 seam；`MockLLMCompletion` 注册进 factory——与本方案同构，最强先例。
- **graphiti**：三个窄 ABC（`LLMClient` / `EmbedderClient` / `CrossEncoderClient`），重排是独立能力（LLM 型 / bge / gemini 多实现）；但外壳只焊在 LLMClient 上，embedder/reranker 裸奔。
- **mem0**：能力 ABC + 工厂注册表，无共享外壳；rerank 失败**静默回退**——正是 R3 契约 2（臂失败 ≠ 空结果、降级必须可观测）禁止的反模式。
- **graphrag-rs**（Rust）：`#[async_trait]` 能力 trait（Embedder / AsyncLanguageModel / CrossEncoder…）+ GAT `type Error` + `BoxedAsync*` 类型擦除别名（DI 形态参考）；反例：`critic/mod.rs` 的 `Critic { client: Arc<OllamaClient> }`——判定焊死在生成客户端上，无法换后端。
- **hindsight**：reranker 通道把 typesafe（=Jev）与本地 cross-encoder / flashrank / jina 并列——**jev 已被当作业界多个判断后端之一**，直接佐证解耦；`LLM_PROVIDER=none` 显式禁用模式（retain 降级、recall 正常、reflect 400）= `DisabledJudge` + 降级语义的先例。

**命名解耦**（硬规则：对外——变量、函数、结构体、模块——一律用 `Judge` 词汇；`jev` 只出现在最底层 wire 层的私有函数/变量/结构体名中）：

- 能力面用 judge 词汇：`JudgeClient`、`JudgeEndpointClient`（compatible endpoint 实现，对外名）、`DisabledJudge`、`JudgeMock`、`degraded: [judge]`、缓存键 `hash(model, op, state, questions)`。
- `jev` 词汇只存在于 `JudgeEndpointClient` 内部的私有 wire 层（`mod wire`：`JevRequest`/`JevResponse`/`JevQuestion`/`JevAnswer`，映射 jev_classify / jev_compare / jev_extract / jev_rerank / jev_find 的线格式——这些结构体与 jev 协议同形，故用 jev 命名）。
- 未来换判断后端：新增一个实现 `JudgeClient` 的类型（对外名用 Judge 词汇）；写读接线、外发门、计量、缓存零改动（AC6 编译期证明）。

## 范围与非目标

- 做：JudgeClient trait + DTO、统一执行外壳（重试/超时/错误分类/计量）、外发门（opt-in + 凭据预检）、jev-compatible 首个后端、缓存、降级语义。
- 不做：训练/微调、多判断后端适配（JudgeClient 先只有 jev-compatible 一个实现；trait 面向能力而非 jev 协议）、自动选择策略（可选扩展，后置）、模型控制面（SuperMew 式 DB 角色分配——库形态用固定模型 id 即可）。

## 接口契约（T1 交付物，DTO/枚举以此为准）

```rust
// src/llm/judge.rs —— 判断能力面（完整定义，测试断言的对象）
pub enum Verdict { Yes, No, Review }            // Review = unresolved，消费侧可区分
pub enum CompareRelation { SameFact, Contradicts, DifferentFacts }
pub enum ExtractStatus { Auto, Review, NotFound }
pub enum ExistsVerdict { Answered, Partial, Absent }

pub struct ClassifyItem  { pub id: String, pub text: String }
pub struct ClassifyClass { pub id: String, pub description: String }
pub struct ClassifyRequest  { pub items: Vec<ClassifyItem>, pub catalog: Vec<ClassifyClass>, pub purpose: Option<String> }
pub struct ClassifyResponse { pub verdicts: Vec<ClassifyVerdict>, pub auto: bool }
pub struct ClassifyVerdict  { pub item_id: String, pub class_id: String, pub verdict: Verdict, pub probability: f64 }

pub struct CompareRequest  { pub passage_a: String, pub passage_b: String, pub aspects: Vec<String> }
pub struct CompareResponse { pub relation: CompareRelation, pub confidence: f64, pub aspects: Vec<AspectVerdict> }
pub struct AspectVerdict   { pub aspect: String, pub relation: CompareRelation, pub confidence: f64 }

pub struct ExtractField    { pub id: String, pub pattern: String, pub description: String }
pub struct ExtractRequest  { pub document: String, pub fields: Vec<ExtractField> }
pub struct ExtractResponse { pub fields: Vec<ExtractResult> }
pub struct ExtractResult   { pub field_id: String, pub value: Option<String>, pub status: ExtractStatus }

pub struct RerankCandidate { pub id: String, pub text: String }
pub struct RerankRequest   { pub query: String, pub candidates: Vec<RerankCandidate> }  // ≤250，超限由调用方排队
pub struct RerankResponse  { pub ranked: Vec<RerankHit> }
pub struct RerankHit       { pub id: String, pub relevance: f64 }

pub struct FindRequest  { pub query: String, pub candidates: Vec<RerankCandidate> }
pub struct FindResponse { pub best: Option<RerankHit>, pub exists_verdict: ExistsVerdict }

pub enum JudgeError { Disabled, Backend(ProviderError), InvalidResponse }
pub type JudgeResult<T> = Result<T, JudgeError>;

#[async_trait]
pub trait JudgeClient: Send + Sync {
    fn model_id(&self) -> String;
    async fn classify(&self, req: &ClassifyRequest) -> JudgeResult<ClassifyResponse>;
    async fn compare(&self, req: &CompareRequest) -> JudgeResult<CompareResponse>;
    async fn extract(&self, req: &ExtractRequest) -> JudgeResult<ExtractResponse>;
    async fn rerank(&self, req: &RerankRequest) -> JudgeResult<RerankResponse>;
    async fn find(&self, req: &FindRequest) -> JudgeResult<FindResponse>;
}

pub struct DisabledJudge;  // 五方法均返回 Err(JudgeError::Disabled)，零网络

// src/llm/judge_mock.rs —— mock 形状（fixture 脚本 + 故障注入同形）
pub struct ScriptedReply { pub op: JudgeOp, pub reply: JudgeReply }
pub enum JudgeReply {
    Classify(ClassifyResponse), Compare(CompareResponse), Extract(ExtractResponse),
    Rerank(RerankResponse), Find(FindResponse),
    Fail(JudgeError),                      // 故障注入：500/超时/invalid_response 走这里
}
pub struct JudgeMock { pub script: Vec<ScriptedReply> }  // 按调用次序消费 script，断言可预测
```

```rust
// src/llm/envelope.rs —— 执行外壳（T0 交付物）
pub enum Op { Embedding, Chat, Judge }

pub struct CallMeta {
    pub model: String,           // 实际模型（后端回显；未回显用配置值并注明）
    pub input_tokens: u64,       // 服务未报 → 0（CallMeta 注明 reported=false）
    pub output_tokens: u64,
    pub candidate_scale: usize,
    pub from_cache: bool,
}

pub enum ProviderError {   // 脱敏：固定码 + 操作族，绝不含上游响应体
    Timeout { op: Op },            // deadline 超时
    RateLimited { op: Op, retry_after: Option<Duration> },  // 429（Retry-After）
    Unavailable { op: Op },        // DNS/TLS/连接失败；408/409/5xx 重试耗尽
    AuthFailed { op: Op },         // 401/403
    InvalidResponse { op: Op },    // 响应畸形/schema 不匹配 → 修调用
    PolicyDenied,                  // 外发门阻断（凭据预检命中）
    Disabled,                      // 空对象被调用（编程错误）
}

pub struct Policy { pub max_attempts: u32, pub initial_backoff: Duration, pub timeout: Duration }
// 默认：max_attempts=3, initial_backoff=500ms（指数 + 抖动）, timeout=60s（对齐 jev 端点 60s deadline）

pub trait MeteringSink: Send + Sync {
    fn record(&self, op: Op, meta: &CallMeta);
    fn snapshot(&self) -> Vec<MeteringRow>;   // 成本报表接口
}
pub struct MeteringRow {  // snapshot 的行
    pub op: Op, pub model: String, pub input_tokens: u64, pub output_tokens: u64,
    pub candidate_scale: usize, pub from_cache: bool,
}
// 默认实现：tracing::info! 结构化事件 + 进程内计数器（Arc<MeteringCounters>，AtomicU64 per (op, model)）

pub async fn execute<T>(
    op: Op, policy: &Policy, gate: &EgressGate, sink: &dyn MeteringSink,
    call: impl Future<Output = Result<T, RawCallError>>,
) -> Result<(T, CallMeta), ProviderError>
// 1. gate.check(op)? —— judge 未 opt-in → PolicyDenied（零调用）
// 2. 循环 attempts：tokio::time::timeout(policy.timeout, call)
//    408/409/429/5xx → 指数退避重试（429 尊重 Retry-After）；其余 → 对应 ProviderError
// 3. sink.record(op, &meta)
```

**失败形态完整映射**（除「超时/401/invalid_response」外的边界）：

| 失败形态 | 归类 |
|---|---|
| DNS/TLS/连接失败 | `Unavailable`（重试策略内） |
| deadline 超时 | `Timeout` |
| HTTP 401/403 | `AuthFailed`（不重试） |
| HTTP 408/409/5xx | 重试；耗尽 → `Unavailable` |
| HTTP 429 | `RateLimited`（带 Retry-After，重试） |
| 响应畸形 / schema 不匹配 | `InvalidResponse`（不重试，修调用） |
| 批超限 / 并发超限 | **非错误**：有界排队（max_pending = 4 × concurrency），超限排队而非截断 |
| 外发门凭据预检命中 | `PolicyDenied`（该批整体阻断） |

**DTO → wire 映射**（`JudgeEndpointClient` 实施表；T2 按此编码/解码，`JudgeMock` 同形）：

 | 操作 | wire `state` | wire `questions` | 响应解析 |
 |---|---|---|---|
 | classify | `{catalog_ids, purpose}` | `items`（`{id, text}` 数组） | `answers.{qid}` → 每 item 的 `class_id` + 概率 → `ClassifyVerdict`；未达阈值 → `Verdict::Review` |
 | compare | `{aspects}` | 两个 passage（`a`/`b`） | `answers.{qid}` → `relation` + `confidence` + 逐 aspect → `CompareResponse` |
 | extract | `{fields: [{id, pattern, description}]}` | `document` | `answers.{qid}` → 每 field 的 `value` + `status` → `ExtractResult` |
 | rerank | `{candidate_ids 有序}` | `query` + 候选（`{id, text}`） | `answers.{qid}` → 每候选 `relevance` → `RerankHit` |
 | find | `{candidate_ids 有序}` | `query` + 候选（`{id, text}`） | `answers.{qid}` → `best` + `exists_verdict` → `FindResponse` |

响应 `usage` 缺失 → `input/output_tokens = 0` 并在 `CallMeta` 注明未报告；`model` 缺失 → 用配置值并注明。

**消费侧语义**（verdict → 调用方行为，T4/T5 实施表）：

 | verdict | classify（写：类型标注） | compare（写：去重/冲突） | extract（写：字段抽取） |
 |---|---|---|---|
 | 判定落定 | item 的 best `class_id` → `Fact.kind`（semantic/episodic/procedural） | `SameFact` → 去重跳过写入；`Contradicts` → 进入冲突处理（supersede 链，src/index/merge.rs 既有逻辑）；`DifferentFacts` → 两者皆写 | `Auto` → `value` 落目标字段 |
 | `Review`/未达阈值 | `trust_state` 保持 `candidate`（安全默认，不推进 `verified`） | 视为未知 → 保守按 `DifferentFacts`（两者皆写） | `Review`/`NotFound` → 字段不落，事实仍写入 |

- 读路径（T5）：`exists_verdict = Absent` → `exists=false`，返回融合序；`Partial`/`Answered` → 返回重排序。

**JudgeEndpointClient 实施规则**（T2；wire 构造以 `@jkudish/jev-mcp` `server.js` 为参考实现逐操作照搬）：

- 鉴权：`Authorization: Bearer <api_key>` 请求头；密钥来自 `api_key_env` 指定的环境变量（值不进配置文件）。
- question 三型（`@typesafe-ai/sdk`）：`noul` = `{type, instructions, criteria:{true,false}}` → 答案 `{noul: 0..1}`；`choice` = `{type, instructions, criteria:{label: description|null}}` → 答案 `{choice: label, probabilities, confidence}`；`score` = `{type, instructions, criteria:[desc...]}` → 答案 `{score: n}`。
- question id 方案：参考实现的键名（`item_{i}` / `rel_{i}` / `f_{i}` / `aspect_{i}` / `overall` / `best` / `exists`）；响应 `answers.{qid}` 按同 id 回填，缺失任一 question id 或多出未知 id → `InvalidResponse`（参考实现 `answer_id_mismatch`）。
- 批拆分：单次 classify/extract 条目数 > `batch_limit` 时，适配器按 `batch_limit` 切分为多个请求（顺序执行，结果按原序拼接）；并发由 envelope 的有界队列约束，适配器本身不并发。
- 响应校验：概率超出 [0,1] 或非有限浮点、概率和 ≠ 1（容差 ±0.05，参考实现 `PROBABILITY_SUM_TOLERANCE`）、`choice` 非 argmax → `InvalidResponse`；`usage` 缺失 → 0/0 并在 `CallMeta` 注明；`model` 缺失 → 配置值并注明。
- HTTP 状态 → 错误：401/403 → `AuthFailed`；429 → `RateLimited`（带 Retry-After）；408/409/5xx → 可重试（envelope 策略）；其余 4xx → `InvalidResponse`（请求本身有问题，不重试）。

**逐操作 wire 构造**（state = 共享上下文一次发送；questions = 每判定一项；参考实现 `server.js` 行号注记）：

| 操作 | wire `state` | wire `questions` | 响应解析要点 |
|---|---|---|---|
| classify | `{purpose, context, classes:[{id:"c0",description}]}`（目录只发一次，criteria 用裸键） | `item_{i}`: `choice({task:"Which class does this item belong to?", item:{id,text}}, {c0:null,…})` | 每 item 答案 `{choice:"c0", probabilities}`；未达阈值 → `Verdict::Review` |
| compare | `{purpose, passage_a, passage_b, aspects}` | `overall`: `choice("Do the two passages state the same underlying fact, contradict each other, or discuss different facts?", {same_fact,contradicts,different_facts})`；`aspect_{i}`: 逐 aspect 同型 | overall + 逐 aspect 独立判定；per-aspect 可与 overall 不一致（消费侧并列呈现） |
| extract | `{purpose, document, fields:[{id:"f0",description,pattern}]}` | `f_{i}`: `choice("Which candidate is the correct value of the field …?", {c0:"Candidate value: …",…, none_of_them:"None of the candidates…"})` | **客户端先跑正则**（沙箱 worker + 硬超时）生成候选；零匹配的字段**不发请求**直接 `NotFound`；候选被截断/超长 → 该字段恒 `Review`（候选宇宙不完整） |
| rerank | `{query}`（query 只发一次） | `rel_{i}`: `noul("Is candidate option_i relevant to the query in the state? Candidate option_i: {text}", {true,…,false,…})` | 任一 `noul` 非有限/[0,1] → 整列 `InvalidResponse`（`ranked: null`，绝不把缺失当 0 分排序） |
| find | `{query, candidates}` | `best`: `choice("Which candidate contains the best answer to: {query}?", {cand_id:null,…})`；`exists`: `noul("Does any candidate address or answer: {query}?", {true,false})` | `exists` 与 `best` 任一缺失 → `InvalidResponse`（缺失不得读作「无匹配」或「无排序」） |

## 契约

### 外发门（默认关闭）

- **opt-in 状态存储**：配置文件 `[judge] enabled`（serde + toml，roadmap §4 技术栈）。库形态无 DB——「workspace 级别」= harness 用初始化时传入的 Config 决定；未开启时 judge 路径零调用（AC1 网络层断言）。
- 门挂在 envelope 的外发 hook 上；per-能力策略：judge 默认关（本节约束）；chat 维持 R1 现状（抽取/报告需要 LLM，非本阶段管辖）；embedding 本地 fastembed 不出网、不过门（未来若换远端实现，接入同壳即纳管）。
- **凭据预检规则**（高置信模式，命中即整批阻断 `PolicyDenied`；预检结果不落日志，密钥值不进任何产物）：
  - `sk-[A-Za-z0-9]{20,}`（OpenAI/兼容 key）、`ghp_[A-Za-z0-9]{36}`（GitHub PAT）、`xox[baprs]-[A-Za-z0-9-]+`（Slack）
  - `-----BEGIN [A-Z ]*PRIVATE KEY-----`（PEM 私钥块）
  - `(password|passwd|secret|api[_-]?key)\s*[:=]\s*["']?[^\s"']{8,}`（赋值形态）
  - 普通代码/路径/业务内容（不含上述锚定模式）在授权 scope 内放行。
1. workspace 级别 opt-in；未开启时**零调用**（计数断言）。
2. 批次预检：高置信 token/密码/私钥/完整凭据 → 直接阻断该批；普通代码/路径/业务内容在授权 scope 内放行。
3. 预检结果不落日志；密钥值不进任何产物。
4. 只发判断所需证据（有界候选），不整段会话外发。

### 调用与成本

| 场景 | 判定操作 | 约束 |
|---|---|---|
| 写：类型标注 | classify | 批量、目录固定（`WRITE_CLASSIFY_CATALOG` 代码常量，src/index/judge.rs：semantic/episodic/procedural——与 `Fact.kind` 文档值一一对应）、`review` 视为 unresolved |
| 写：去重/冲突 | compare | 仅对 embedding 近邻调用：cosine ≥ 0.85 且每 scope 向量臂 top-10（`compare_min_cosine` 配置化） |
| 写：字段抽取 | extract | regex 限定候选；`status`/`reason` 必读 |
| 读：精排 | rerank | ≤250 候选；近零分整列 → 视为 miss |
| 读：答案判定 | find | 必读 `exists_verdict`，无答案不强选 |

（jev 后端映射：classify→`jev_classify`、compare→`jev_compare`、extract→`jev_extract`、rerank→`jev_rerank`、find→`jev_find`。操作名与约束属于能力面，不随后端变。）

- 每次请求记录：实际模型、usage（服务未报则标 0/0 并注明）、候选规模。（envelope 统一记录，三能力同格式，落 MeteringSink）
- 结果缓存 key：`hash(model, op, state, questions)`；同一判断不重掷。各分量定义：
  - `model`：`JudgeClient::model_id()`
  - `op`：五操作名之一
  - `state`：操作专属非文本上下文的 canonical JSON——classify = `{catalog_ids, purpose}`；compare = `{aspects}`；extract = `{field_ids + patterns}`；rerank/find = `{candidate_ids 有序}`
  - `questions`：待判定文本的 canonical JSON（classify = items；compare = 双 passage；extract = document；rerank/find = query + 候选摘要）
- 单批上限（`batch_limit`，默认 64，约束 classify/extract 的条目批）与并发（`concurrency`，默认 4）配置化；超限排队而非截断（有界队列 max_pending = 4 × concurrency）。rerank/find 的候选上限另计：≤250（`JEV_CANDIDATE_CAP`，读路径已钳位）。

### 降级语义

- Judge 后端不可用（超时/401/invalid_response，见失败形态映射表）：写路径停止推进水位（R2 契约）；读路径跳过 rerank，返回融合序 + `degraded: [judge]`。
- 不静默换成普通 LLM 摘要——写路径宁可滞后。（mem0 的静默回退为反例）
- 区分判定失败与传输失败：`invalid_response` = 模型答错（修调用）；超时/401/`fetch failed` = 端点不健康（查端点，不动提示词与判断逻辑）。上游响应体不进客户端可见错误。

## 任务

- R4.T0 执行外壳：`llm/envelope.rs`——重试（408/409/429/5xx、指数退避）、超时、脱敏错误分类、计量行接口（`MeteringSink` + 默认实现）、外发门 hook；`LlmClient` 现路径迁入同壳（chat 行为不回归，先行受益）。
- R4.T1 判断能力面：`llm/judge.rs`（§接口契约的 trait + DTO + `DisabledJudge`）+ `llm/judge_mock.rs`（`JudgeMock`，含 `Fail` 故障注入）。
- R4.T2 jev 后端：`llm/judge_jev.rs` 的 `JudgeEndpointClient` 实现 `JudgeClient`（compatible endpoint 封装；线格式 `{model, state, questions}` → `{answers, usage, model}`；逐操作 wire 构造、question 三型（noul/choice/score）、question id 方案、批拆分、响应校验、HTTP 状态映射全部按 §接口契约「JudgeEndpointClient 实施规则」与「逐操作 wire 构造」两节实施——参考实现 `@jkudish/jev-mcp` `server.js`；`jev` 词汇仅限其私有 `mod wire` 的 `JevRequest`/`JevResponse`/`JevQuestion`/`JevAnswer`；超时/重试策略与 Jev 参考实现一致；错误字符串不泄上游体）。
- R4.T3 外发门：opt-in 状态机（Config `[judge] enabled`）+ 凭据预检（§外发门规则）+ 零外发测试。
- R4.T4 写侧接线：**新建** `src/index/judge.rs`（代码库中不存在 R2.T4 stub——本任务是从零建调用点，非替换）。**交付物 = 纯函数**（无 store I/O）：`judge_write_candidates(candidates: Vec<FactCandidate>, near_neighbors: Vec<Vec<ComparePassage>>, client: &dyn JudgeClient) -> Result<Vec<JudgedCandidate>, JudgeError>`——只做 ②③④（classify/compare/extract）。`FactCandidate = { text: String, scope: Scope, source_refs: Vec<String> }`，`ComparePassage = { text: String, fact_id: FactId }`，`JudgedCandidate = { candidate, classify: Verdict, compare: Vec<CompareResult>, extract: BTreeMap<String, ExtractOutcome> }`。**治理写网关（调用方，C01/C06 落地）职责**：① 派生候选（初始规则 = 1 个 closed turn = 1 个候选，turn 文本即 fact 文本；话题切分后置）② 价值筛选（复用 `ChunkValueScorer`，参考语料 = 本批候选 + 已写入事实文本——网关查 store，低值 turn 零 judge 调用）③ 查近邻（`FactVectorStore` 同 scope ANN top-10，cosine ≥ `compare_min_cosine` 默认 0.85 预筛）④ 调 `judge_write_candidates` ⑤ 落库（`FactStore::put`，src/store/facts.rs:84——当前零调用——+ R1/R2 三库提交协议）。**失败语义**：judge 失败 → turn 提交中止（fact put 与 chunk 提交均不发生，watermark 不动——AC5 断言对象）。各步骤细节：② `classify` 用 `WRITE_CLASSIFY_CATALOG` 代码常量（与 `Fact.kind` 一一对应三类：`semantic` 可复用知识事实 / `episodic` 事件会话片段 / `procedural` 操作步骤，各含一句判定标准 description）→ ③ `compare` 对 `near_neighbors[i]` 逐条 compare（候选尚无 FactId，近邻均为已写入事实）→ ④ `extract` 用 `WRITE_EXTRACT_FIELDS` 代码常量（初始集：`error_code` / `path` / `task_id` 三类，各含 regex 模式与 description；抽取值追加到 fact 文本的检索副本供词法臂精确命中（R3 AC1 召回基础），不新增 schema 列；regex 零匹配的字段恒 `NotFound`，不触发请求）。`SessionIngest::new`（ingest.rs:65）增 `judge: Arc<dyn JudgeClient>` 字段（默认 `DisabledJudge`，随网关启用）。
- R4.T5 读侧接线：R3 融合后 rerank + exists 判定。精确管线位置（与 03 文档流程一致）：`rrf_fuse` → 时间臂稳定重排（03 T5 已落地）→ **judge rerank**（候选 = 融合结果，`RerankCandidate.id = FactId`，≤250 已钳位；按 `RerankHit.relevance` 重排；近零分整列 → 视为 miss，保持融合序）→ **find 式 exists 判定**（对重排序后的候选，`exists_verdict = Absent` → `exists=false`）→ `top_k` 截断到预算。`MemoryRecall` 加 `rerank: Option<Arc<dyn JudgeClient>>`，与现有 vector/lexical/edges 三个 Option 臂同构；`ArmName` 增 `Judge` 变体（失败标 `degraded: [judge]`）。
- R4.T6 缓存与计量：请求级记录（`MeteringSink::record`）+ 去重缓存（`CachedJudge` 镜像 `CachedLlm`：缓存行键格式 `judge:{op}:{hash}`，同一 `KvStore` 实例与同一注入点；五操作全部走缓存——判断幂等，同一 state+questions 必同果；键分量见 §调用与成本）+ 成本报表接口（`MeteringSink::snapshot`）。`MeteringSink` 由 `JudgeClient` 构造点注入（与 `judge` 字段同处 `SessionIngest`/`MemoryRecall` 的构造参数）；默认实现 = `tracing::info!` 结构化事件 + 进程内 `AtomicU64` 计数器。
- R4.T7 降级：故障注入（`JudgeMock` 的 `Fail` 脚本：断网/500/格式错）走降级路径，断言行为与标记。

## 验收与测试矩阵

| AC | 具名测试 | 具体断言 |
|---|---|---|
| R4.AC1 未 opt-in 零外发 | `tests/judge_disabled_zero_egress.rs` | 未 opt-in 跑全测试套件：网络层（reqwest mock/本地 httptest 桩）断言零外发；`DisabledJudge` 为默认注入；调用计数 = 0 |
| R4.AC2 凭据阻断 | `tests/judge_credential_block.rs` | 含 §外发门规则任一模式的批次被 `PolicyDenied` 阻断；日志/产物扫描无密钥值 |
| R4.AC3 三状态可区分 | `tests/judge_states_distinguishable.rs` | `Verdict::Review` / `JudgeError::InvalidResponse` / `ExistsVerdict::Absent` 在消费侧各有独立分支与断言，互不混淆 |
| R4.AC4 缓存不重掷 | `tests/judge_cache_no_reroll.rs` | 相同 `hash(model, op, state, questions)` 请求零第二调用（mock 计数 = 1）；计量行含实际模型 |
| R4.AC5 断网降级 | `tests/judge_degradation.rs` | `Fail(Backend(Unavailable))` 注入：写路径水位停（watermark 不前进）、读路径返回融合序 + `degraded: [judge]`；断言无静默 LLM 替换 |
| R4.AC6 后端可换 | `tests/judge_backend_swap.rs` | `JudgeMock` ↔ `JudgeEndpointClient` 替换注入同一接线函数，编译不变、调用序列一致 |
| T0 外壳正确性 | `tests/envelope_retry.rs` | 408/429/5xx 重试至成功；401/畸形响应不重试；超时归类 `Timeout`；退避序列符合指数+抖动 |

## 验证与烟测

夹具 `JudgeMock` 验证协议消费与接线；真实 jev 冒烟在授权环境单跑（`JudgeEndpointClient`），记录模型与用量（`MeteringSink::snapshot`），仅用合成的隔离数据。

## 配置汇总（T3/T6，config toml）

```toml
[judge]
enabled = false            # 外发门 opt-in（workspace 级；Config 传入，无 DB）
endpoint = ""              # jev-compatible endpoint
api_key_env = "JEV_API_KEY"  # 环境变量键名；密钥值不进配置文件
model = "jev-latest"       # 固定模型版本（进缓存键与计量行）
timeout_seconds = 60       # 单次 deadline
max_attempts = 3           # 重试上限
initial_backoff_ms = 500   # 指数退避基数（+抖动）
batch_limit = 64           # 单批上限（超限排队）
concurrency = 4            # 并发上限（max_pending = 4 × concurrency）
compare_min_cosine = 0.85  # compare 近邻阈值（每 scope 向量臂 top-10）
```

## 接续记录

- 已完成（2026-10-12 实施落地，T0–T7 全链；T4 纯函数 + JudgeMock 测试已交付，接线随 C01/C06 治理写网关）：
  - T0 `src/llm/envelope.rs`：执行外壳——重试（408/409/429/5xx、指数退避 + 抖动）、deadline 超时、脱敏 `ProviderError`（固定码 + 操作族，不含上游体）、`MeteringSink`/`DefaultMeteringSink`/`MeteringRow`、`EgressGate`（opt-in + 凭据预检 5 锚定模式）。`LlmClient`（openai.rs）已迁入同壳，chat 行为不回归（envelope 单测 + 既有 chat 测试全绿）。
  - T1 `src/llm/judge.rs`：`JudgeClient` trait（classify/compare/extract/rerank/find 五操作）+ 全套 DTO + `DisabledJudge`；`src/llm/judge_mock.rs`：`JudgeMock`（脚本按调用次序消费、`Fail` 故障注入、op 不匹配/脚本耗尽 panic 断言）；`src/index/judge.rs`：`judge_write_candidates` 纯函数（②③④：classify/compare/extract，无 store I/O；`WRITE_CLASSIFY_CATALOG` semantic/episodic/procedural 对齐 `Fact.kind`；`WRITE_EXTRACT_FIELDS` 正则候选）。
  - T2 `src/llm/judge_jev.rs`：`JudgeEndpointClient`——私有 `mod wire`（`JevRequest`/`JevQuestion`/`JevResponse`/`JevAnswer`，仅此处用 jev 词汇）；question 三型 noul/choice/score 按 `@typesafe-ai/sdk` 源码；逐操作 wire 构造按 `@jkudish/jev-mcp` `server.js` 照搬（classify `item_{i}` + 裸键 criteria；compare `overall` + `aspect_{i}`；extract `f_{i}` + `none_of_them` 候选 + 客户端正则先行、零匹配不发请求；rerank `rel_{i}` noul；find `best` + `exists`）；批拆分（batch_limit 切分、顺序执行、原序拼接）；响应校验（id 集合精确匹配、概率 [0,1] 有限、概率和容差 0.01+1e-12——参考实现 `PROBABILITY_SUM_TOLERANCE` 实测值，非文档草稿的 ±0.05、choice 须为 argmax）；HTTP 状态映射（401/403→AuthFailed 不重试；429→RateLimited 带 Retry-After；408/409/5xx 重试；其余 4xx→InvalidResponse）；`concurrency` 信号量（排队而非截断）。
  - T3 `EgressGate`（envelope.rs）：opt-in 状态机（`EgressGate::new(judge_enabled)`——库无 Config 加载器，workspace 级 opt-in 由 harness 构造时传入；doc 配置汇总的 toml 键 = `JudgeEndpointConfig`/`Policy` 字段一一对应）；凭据预检 5 锚定模式命中即整批 `PolicyDenied`；预检结果不落日志。
  - T5 `src/query/facts.rs`：`MemoryRecall.judge: Option<Arc<dyn JudgeClient>>`（doc 草稿名 `rerank`——实际字段服务 rerank + find 两操作，用 judge 命名更诚实）；`ArmName::Judge`（Display `"judge"`，degraded 词表）；精排管线位置与 03 文档一致（`rrf_fuse` → 时间臂重排 → judge rerank → find 式 exists → top_k 截断）；近零分整列 → 视为 miss 保持融合序；`exists_verdict = Absent` → `exists=false` 返回空候选；失败标 `degraded: ["judge"]`（一次——两调用共享后端）。
  - T6 `src/llm/judge_cache.rs`：`CachedJudge` 镜像 `CachedLlm`（键 `judge:{op}:{md5(model, request)}`——model 进键，换模型即 miss；同一 `KvStore` 注入点；五操作全走缓存）；计量经 `MeteringSink`（envelope 统一 record，实际模型/usage 未报 0/0/候选规模）。
  - T7 `tests/judge_jev_wire.rs`：裸 TCP mock 端点（先读尽请求再响应）——断网 → `Backend(Unavailable)`；HTTP 500 → 按策略重试后降级；畸形体 → `InvalidResponse`；未 opt-in → 连接级零外发（accepted connections = 0）。
  - 验收矩阵落点：R4.AC1 = `judge_jev_wire.rs::zero_egress_until_opted_in`（连接级）+ envelope 单测 `gate_denies_judge_until_opted_in`；R4.AC2 = envelope 单测 `credential_precheck_blocks_high_confidence_patterns`；R4.AC3 = `index/judge.rs` 单测（Review 未达阈值/未知 class/unknown item id）+ `judge_jev.rs` 校验单测 + `memory_recall.rs` find-absent 分支；R4.AC4 = `judge_cache.rs` 单测 `a_hit_does_not_reroll_the_judgment`（计数 = 1）+ 计量行含实际模型；R4.AC5 = `judge_jev_wire.rs` 三故障注入 + `memory_recall.rs::judge_failure_degrades_to_the_fused_order` + `index/judge.rs::judge_failure_aborts_the_batch`（写路径水位停 = 整批中止）；R4.AC6 = 构造期证明——接线函数（`judge_write_candidates`/`MemoryRecall`）只依赖 `dyn JudgeClient`，`JudgeMock` ↔ `JudgeEndpointClient` 替换是类型级无操作，两后端均经同一接线被既有测试覆盖；T0 外壳 = envelope 单测（408/429/5xx 重试至成功、401/畸形不重试、超时归类 Timeout、退避序列）。
  - 偏差记录：(1) 文档「接口契约」节的 DTO→wire 简表被「逐操作 wire 构造」节取代——实施以后者（参考实现精确形状）为准；(2) 概率和容差用参考实现实测 0.01+1e-12（文档草稿 ±0.05 与参考源码不符）；(3) AC 测试按组件落点而非按 AC 建独立文件（每 AC 断言在对应模块测试内）；(4) `MemoryRecall` 字段名 `judge`（doc 草稿 `rerank`）；(5) 库无 Config/toml 加载器（既有形态）——opt-in 经 `EgressGate::new(bool)` 注入，配置键形状与 doc 配置汇总一致。
- 未完成：T4 接线（治理写网关 C01/C06 落地时接入：派生候选 → 价值筛选 → 近邻查找 → `judge_write_candidates` → 提交/停水位）；真实 jev 冒烟（授权环境单跑）。
- 下一动作：PR #7（feat/r4-judge-integration，squash 46a37cb）已合并——R4 收尾完成；本地 main 待下次 fetch 同步（合并时 GitHub 网络缓慢，fetch 超时，远端已确认 MERGED）。随后 05-reflect。
