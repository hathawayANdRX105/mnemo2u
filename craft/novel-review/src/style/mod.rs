//! 风格化生成模块（issue #5）：风格画像、设定卡、DND 文字冒险。
//!
//! 设计参照 `todo/refs/`：
//! - SillyTavern Character Card v2/v3 → 设定卡结构化 + 关键词动态注入；
//! - RPG-OS → 持久化状态文件，确定性代码管规则（骰子/HP/技能），
//!   LLM 只管叙述；
//! - Azgaar → 可选的地图/世界生成辅助。
//!
//! 全部逻辑为确定性纯函数/文件操作，可离线测试；LLM 叙述通过
//! `Narrator` trait 注入，测试用假实现。

pub mod adventure;
pub mod cards;
pub mod cli;
pub mod profile;

pub use cards::{
    default_cards_dir, duplicate_free, render_cards, select_cards, SettingCard, SettingCardSet,
};
pub use profile::{
    count_kaomoji, default_styles_dir, kaomoji_density_per_10k, kaomoji_over_cap, select_kaomoji,
    StyleProfile, StyleProfileSet,
};
