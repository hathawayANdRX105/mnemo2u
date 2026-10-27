# 本地工作纪律

## 编译与测试一律交给 CI

**不要在本地跑 `cargo build` / `cargo test` / `cargo clippy`。**

本地编译和测试很慢（全量 `--all-targets` 要 20 分钟以上），而且占用 CPU 挤占其他工作。
正确做法：

1. 写完代码 → `git commit` → `git push`
2. `gh pr create` 开 PR
3. `gh pr checks <N>` 或 `gh run watch` 看 CI 结果
4. CI 红了 → 读日志 → 修 → 再 push

需要本地确认某个具体行为时，**只跑单个测试目标**（`cargo test --test <name>`），
不要跑 `--all-targets` 全量。

CI 跑什么（`.github/workflows/ci.yml`）：

| step | 内容 |
|---|---|
| `fmt` | `cargo fmt --all -- --check` |
| `test` | `cargo clippy --all-targets -- -D warnings` + `cargo test --all-targets --no-fail-fast` + 三个 smoke 探针 |

**注意 CI 不单独跑 clippy 的 `cargo fmt`**，但 clippy 在 test job 里（`-D warnings`）。
本地改动提交前至少跑一次 `cargo fmt --all`（很快，只格式化不编译），否则 CI 的 fmt step 会红。

## 其它

- 提交身份固定 `hathawayANdRX105`（本地邮箱 `2635254302@qq.com` 或 GitHub 的
  `61958173+hathawayANdRX105@users.noreply.github.com`）。
- 提交前跑 `canon pre-commit` / `canon pre-push`（仓库里有 `.githooks/canon`）。
- 合并只走 squash：`gh pr merge <N> --squash --delete-branch`。
- 不留兼容垫片：改了接口就同步迁移所有调用方。
