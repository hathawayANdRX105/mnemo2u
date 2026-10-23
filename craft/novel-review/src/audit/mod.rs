//! 审查（audit）模块：`src/audit/*` 的 Rust 移植。
//!
//! - [`draft`]：`audit.draft` 完整分析（9 个规则节 + 结构/语料/疲劳等全部顶层节）。
//! - [`plan`] / [`concept`]：大纲与概念卡审计。

pub mod concept;
pub mod draft;
pub mod plan;
