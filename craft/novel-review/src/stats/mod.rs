//! 统计（stats）模块：`src/stats/*` 的 Rust 移植。
//!
//! - [`plan`]：`stats.plan`（arc/story/chapter plan 的镜像 markdown 统计树）。
//! - [`concept`]：`stats.concept`（概念卡镜像 markdown 统计树）。

/// `Counter`（首现序稳定 tie-break），供跨模块复用（如 `consistency` 反馈统计）。
pub use draft::Ctr;
pub mod concept;
pub mod draft;
pub mod plan;
