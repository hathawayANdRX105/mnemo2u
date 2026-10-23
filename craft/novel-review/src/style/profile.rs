//! 风格画像：生成端注入的风格定义（一等公民配置）。
//!
//! 对应 `configs/styles/<name>.yaml`：
//! - 风格画像：词汇偏好、句长、标点、节奏、人称、禁忌；
//! - 示例语料（few-shot）；
//! - 特征规则：期望密度 vs 禁忌密度（复用 `review.yaml` 形状）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 默认风格配置目录。
#[must_use]
pub fn default_styles_dir() -> PathBuf {
    PathBuf::from("configs/styles")
}

/// 单个风格画像。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleProfile {
    /// 风格 id（文件名去扩展名）。
    pub id: String,
    /// 展示名。
    pub name: String,
    /// 风格描述（注入生成 prompt 的画像段）。
    pub description: String,
    /// 期望出现的特征（如「短句连发」「对话驱动」）。
    #[serde(default)]
    pub expect: Vec<String>,
    /// 禁忌特征（如「解释腔」「流水账」）。
    #[serde(default)]
    pub forbid: Vec<String>,
    /// few-shot 示例段落。
    #[serde(default)]
    pub samples: Vec<String>,
    /// 颜文字映射表：情绪 -> 候选颜文字（用于生成 prompt 注入与密度管控）。
    #[serde(default)]
    pub kaomoji: BTreeMap<String, Vec<String>>,
    /// 术语表：术语 -> 释义（DND 等游戏风格注入）。
    #[serde(default)]
    pub glossary: BTreeMap<String, String>,
    /// 颜文字频率上限（每万字，防滥用）。
    #[serde(default = "default_kaomoji_cap")]
    pub kaomoji_cap_per_10k: usize,
}

fn default_kaomoji_cap() -> usize {
    40
}

/// 风格集合（一个 yaml 文件可含多风格）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleProfileSet {
    #[serde(default)]
    pub styles: Vec<StyleProfile>,
}

impl StyleProfileSet {
    /// 从 yaml 文件加载。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取风格配置失败: {}", path.display()))?;
        Self::from_yaml(&text)
    }

    /// 从 yaml 文本解析。
    pub fn from_yaml(text: &str) -> Result<Self> {
        serde_yaml::from_str(text).context("风格配置 YAML 解析失败")
    }

    /// 按 id 查找。
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&StyleProfile> {
        self.styles.iter().find(|s| s.id == id)
    }

    /// 渲染注入生成 prompt 的风格画像文本（纯函数，可测试）。
    #[must_use]
    pub fn render_prompt(&self, id: &str) -> Option<String> {
        let profile = self.find(id)?;
        let mut out = format!("# 风格：{}\n\n{}", profile.name, profile.description);
        if !profile.expect.is_empty() {
            out.push_str("\n\n## 期望特征\n");
            for item in &profile.expect {
                out.push_str(&format!("- {item}\n"));
            }
        }
        if !profile.forbid.is_empty() {
            out.push_str("\n## 禁忌特征\n");
            for item in &profile.forbid {
                out.push_str(&format!("- 避免：{item}\n"));
            }
        }
        if !profile.samples.is_empty() {
            out.push_str("\n## 示例\n");
            for sample in &profile.samples {
                out.push_str(&format!("{sample}\n\n"));
            }
        }
        if !profile.kaomoji.is_empty() {
            out.push_str(&format!(
                "\n## 颜文字映射\n仅在角色吐槽、内心独白等轻快处使用；全文不超过每万字 {} 个。\n",
                profile.kaomoji_cap_per_10k
            ));
            for (emotion, faces) in &profile.kaomoji {
                out.push_str(&format!("- {emotion}: {}\n", faces.join(" ")));
            }
        }
        if !profile.glossary.is_empty() {
            out.push_str("\n## 术语表\n");
            for (term, meaning) in &profile.glossary {
                out.push_str(&format!("- {term}: {meaning}\n"));
            }
        }
        Some(out)
    }
}

/// 稳定字符串散列（FNV-1a 64 位），用于确定性颜文字挑选。
fn stable_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 按情绪与上下文确定性地选一个颜文字（同上下文永远同结果，可复现）。
///
/// 语义贴合度筛选留给 LLM 的 `find`（见 `tools::judge` 同端点约定）；
/// 本函数保证离线、确定性、可测试。
#[must_use]
pub fn select_kaomoji<'a>(
    profile: &'a StyleProfile,
    emotion: &str,
    context: &str,
) -> Option<&'a str> {
    let faces = profile.kaomoji.get(emotion)?;
    if faces.is_empty() {
        return None;
    }
    let index = (stable_hash(context) % faces.len() as u64) as usize;
    faces.get(index).map(String::as_str)
}

/// 统计文本中颜文字出现的总次数（映射表内全部候选都算，重复出现重复计）。
#[must_use]
pub fn count_kaomoji(profile: &StyleProfile, text: &str) -> usize {
    profile
        .kaomoji
        .values()
        .flatten()
        .filter(|face| !face.is_empty())
        .map(|face| text.matches(face.as_str()).count())
        .sum()
}

/// 每万字颜文字密度（纯函数，可测试）。
#[must_use]
pub fn kaomoji_density_per_10k(profile: &StyleProfile, text: &str) -> f64 {
    let char_count = text.chars().filter(|c| !c.is_whitespace()).count();
    if char_count == 0 {
        return 0.0;
    }
    count_kaomoji(profile, text) as f64 / char_count as f64 * 10000.0
}

/// 是否超出风格画像的颜文字频率上限（防滥用判定，纯函数）。
#[must_use]
pub fn kaomoji_over_cap(profile: &StyleProfile, text: &str) -> bool {
    kaomoji_density_per_10k(profile, text) > profile.kaomoji_cap_per_10k as f64
}
