# R4 Jev 全链（判断接入与外发门）

**状态**：待实施。
**目标**：把 Jev 的判断能力接入写读两侧（写：classify/compare/extract；读：rerank/find），并落地外发门与成本控制；无 Jev 时全库可用、显式降级。
**前置**：[R2](02-write-path.md)、[R3](03-retrieval.md)验收。
**下一阶段**：[05-reflect](05-reflect.md)。

## 范围与非目标

- 做：compatible 协议客户端、固定模型版本、写读两侧工具接线、外发门（opt-in + 凭据预检）、缓存、降级语义。
- 不做：训练/微调、多 provider 适配（先 single compatible endpoint）、自动选择策略（可选扩展，后置）。

## 契约

### 外发门（默认关闭）

1. workspace 级别 opt-in；未开启时**零调用**（计数断言）。
2. 批次预检：高置信 token/密码/私钥/完整凭据 → 直接阻断该批；普通代码/路径/业务内容在授权 scope 内放行。
3. 预检结果不落日志；密钥值不进任何产物。
4. 只发判断所需证据（有界候选），不整段会话外发。

### 调用与成本

| 场景 | 工具 | 约束 |
|---|---|---|
| 写：类型标注 | `jev_classify` | 批量、目录固定、`review` 视为 unresolved |
| 写：去重/冲突 | `jev_compare` | 仅对 embedding 近邻调用 |
| 写：字段抽取 | `jev_extract` | regex 限定候选；`status`/`reason` 必读 |
| 读：精排 | `jev_rerank` | ≤250 候选；近零分整列 → 视为 miss |
| 读：答案判定 | `jev_find` | 必读 `exists_verdict`，无答案不强选 |

- 每次请求记录：实际模型、usage（服务未报则标 0/0 并注明）、候选规模。
- 结果缓存 key：`hash(model, tool, state, questions)`；同一判断不重掷。
- 单批上限与并发配置化；超限排队而非截断。

### 降级语义

- Jev 不可用（超时/401/invalid_response）：写路径停止推进水位（R2 契约）；读路径跳过 rerank，返回融合序 + `degraded: [jev]`。
- 不静默换成普通 LLM 摘要——写路径宁可滞后。

## 任务

- R4.T1 客户端：compatible endpoint 封装（超时/重试策略与 Jev 参考实现一致：408/409/429/5xx 重试，错误字符串不泄上游体）。
- R4.T2 外发门：opt-in 状态机 + 凭据预检 + 零外发测试。
- R4.T3 写侧接线：R2.T4 的 stub 换成真实调用（classify/compare/extract）。
- R4.T4 读侧接线：R3 融合后 rerank + exists 判定。
- R4.T5 缓存与计量：请求级记录 + 去重缓存 + 成本报表接口。
- R4.T6 降级：故障注入（断网/500/格式错）走降级路径，断言行为与标记。

## 验收

- [ ] R4.AC1 未 opt-in：全测试套件零外发（网络层断言）。
- [ ] R4.AC2 凭据命中批次被阻断；密钥不出现在日志/产物（扫描测试）。
- [ ] R4.AC3 `review`/`invalid_response` 与「无答案」三种状态在消费侧可区分，各有测试。
- [ ] R4.AC4 缓存命中不重掷（相同请求零第二调用）；计量记录含实际模型。
- [ ] R4.AC5 断网降级：写停水位/读标 degraded，不静默替换。

## 验证与烟测

夹具 mock 服务验证协议消费；真实 Jev 冒烟在授权环境单跑，记录模型与用量，仅用合成的隔离数据。

## 接续记录

- 已完成：无。
- 未完成：T1–T6。
- 下一动作：R3 验收后先定义客户端 trait 与 mock 服务形状。
