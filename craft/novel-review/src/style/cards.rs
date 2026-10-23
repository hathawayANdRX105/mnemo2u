//! 设定卡：结构化世界/人物/规则定义 + 关键词动态注入。
//!
//! 参照 SillyTavern Character Card v2/v3 与 World Info：
//! - 卡片含 id/类型/关键词/内容；
//! - `select_cards` 按当前场景文本命中的关键词选出相关卡片，
//!   注入生成上下文（避免全文设定塞爆上下文）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 默认设定卡目录。
#[must_use]
pub fn default_cards_dir() -> PathBuf {
    PathBuf::from("configs/cards")
}

/// 卡片类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    Character,
    World,
    Rule,
    Item,
}

/// 一张设定卡。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingCard {
    /// 卡片 id（唯一）。
    pub id: String,
    /// 类型。
    pub kind: CardKind,
    /// 触发关键词：场景文本命中任一关键词即注入。
    #[serde(default)]
    pub keywords: Vec<String>,
    /// 卡片正文（注入 prompt 的设定内容）。
    pub content: String,
}

/// 设定卡集合。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingCardSet {
    #[serde(default)]
    pub cards: Vec<SettingCard>,
}

impl SettingCardSet {
    /// 从 yaml 文件加载。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取设定卡失败: {}", path.display()))?;
        Self::from_yaml(&text)
    }

    /// 从 yaml 文本解析。
    pub fn from_yaml(text: &str) -> Result<Self> {
        serde_yaml::from_str(text).context("设定卡 YAML 解析失败")
    }

    /// 按 id 查找。
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&SettingCard> {
        self.cards.iter().find(|c| c.id == id)
    }

    /// id 唯一性校验（配置 lint，可测试）。
    #[must_use]
    pub fn duplicate_ids(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut dups = Vec::new();
        for card in &self.cards {
            if !seen.insert(card.id.clone()) {
                dups.push(card.id.clone());
            }
        }
        dups.sort_unstable();
        dups
    }
}

/// 快捷校验：id 无重复（配置 lint，可测试）。
#[must_use]
pub fn duplicate_free(cards: &SettingCardSet) -> bool {
    cards.duplicate_ids().is_empty()
}

/// 按场景文本选出命中的卡片（命中任一关键词即选中，按卡片序稳定输出）。
#[must_use]
pub fn select_cards<'a>(cards: &'a SettingCardSet, scene_text: &str) -> Vec<&'a SettingCard> {
    cards
        .cards
        .iter()
        .filter(|card| {
            card.keywords
                .iter()
                .any(|kw| !kw.is_empty() && scene_text.contains(kw.as_str()))
        })
        .collect()
}

/// 把选中卡片渲染为注入 prompt 的设定块（纯函数，可测试）。
#[must_use]
pub fn render_cards(cards: &[&SettingCard]) -> String {
    if cards.is_empty() {
        return String::new();
    }
    let mut out = String::from("# 相关设定\n\n");
    for card in cards {
        out.push_str(&format!("## {}\n{}\n\n", card.id, card.content));
    }
    out
}
