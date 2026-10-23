//! `src/reports` 报告生成模块。
//!
//! - `alignment`：章节施工图 ↔ 草稿对齐信号，
//!   由 scorecard 等报告消费（后续 reports 分片可直接复用）。
//! - `scorecard`：`reports-scorecard` 子命令。
//!
//! 后续分片（catalog / backlog / kit / learning / profiles / workspace）
//! 在本文件追加 `pub mod` 声明，不改动已有模块。

pub mod alignment;
pub mod backlog;
pub mod catalog;
pub mod kit;
pub mod learning;
pub mod profiles;
pub mod scorecard;
pub mod workspace;
