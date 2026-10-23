//! DND 文字冒险：状态机 + 确定性规则 + 可插拔叙述。
//!
//! 参照 RPG-OS：
//! - 状态存可读文件（JSON），回合间持久；
//! - 骰子、HP、技能判定等规则由确定性代码执行（防 LLM 幻觉破坏规则）；
//! - LLM 只负责叙述（`Narrator` trait，测试用假实现）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// 默认冒险状态目录。
#[must_use]
pub fn default_adventure_dir() -> PathBuf {
    PathBuf::from("adventure")
}

/// 确定性伪随机（xorshift），种子存在状态里，保证可复现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// 由种子创建。
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        let state = if seed == 0 { 1 } else { seed };
        Self { state }
    }

    /// 取 [1, sides] 的骰子结果，并推进内部状态。
    pub fn roll(&mut self, sides: u32) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        ((self.state % u64::from(sides)) + 1) as u32
    }
}

/// 角色属性。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    pub hp: i32,
    pub max_hp: i32,
    pub attack: i32,
    pub defense: i32,
}

/// 敌人实体（战斗中登场，HP 归零后清除）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enemy {
    pub name: String,
    pub hp: i32,
    pub max_hp: i32,
}

/// 冒险状态（可序列化为可读 JSON 文件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdventureState {
    /// 世界名。
    pub world: String,
    /// 当前场景描述（确定性部分，如位置）。
    pub scene: String,
    pub character: Character,
    /// 任务/世界旗标（键值状态）。
    #[serde(default)]
    pub flags: std::collections::BTreeMap<String, bool>,
    /// 物品栏。
    #[serde(default)]
    pub inventory: Vec<String>,
    /// 当前敌人（无战斗时为 None）。
    #[serde(default)]
    pub enemy: Option<Enemy>,
    /// RNG 种子状态。
    pub rng: Rng,
    /// 回合计数。
    pub turn: u32,
    /// 注入的设定/风格上下文（设定卡渲染文本，持久化）。
    #[serde(default)]
    pub lore: String,
}

/// 一次动作的判定结果（确定性规则部分）。
#[derive(Debug, Clone, PartialEq)]
pub enum ActionOutcome {
    /// 攻击：目标、骰点、命中与否、伤害。
    Attack {
        enemy: String,
        roll: u32,
        hit: bool,
        damage: i32,
    },
    /// 遭遇敌人。
    Encounter { enemy: String, hp: i32 },
    /// 无可攻击目标。
    NoTarget,
    /// 移动/观察：进入新场景。
    Move { to: String },
    /// 物品操作。
    Item { item: String },
    /// 未知动作（交给叙述层发挥）。
    Unknown,
}

/// 叙述器 trait：由 LLM 实现（或测试替身）。
pub trait Narrator {
    /// 根据状态与动作结果生成叙述文本。
    fn narrate(&self, state: &AdventureState, action: &str, outcome: &ActionOutcome) -> String;
}

impl AdventureState {
    /// 新建状态。
    #[must_use]
    pub fn new(world: impl Into<String>, scene: impl Into<String>, character: Character) -> Self {
        Self {
            world: world.into(),
            scene: scene.into(),
            character,
            flags: std::collections::BTreeMap::new(),
            inventory: Vec::new(),
            enemy: None,
            rng: Rng::new(0x5EED_1234),
            turn: 0,
            lore: String::new(),
        }
    }

    /// 从 JSON 文件加载。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取冒险状态失败: {}", path.display()))?;
        Self::from_json(&text)
    }

    /// 从 JSON 文本解析。
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("冒险状态 JSON 解析失败")
    }

    /// 序列化为可读 JSON。
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 保存到文件。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建冒险目录失败: {}", parent.display()))?;
        }
        std::fs::write(path, self.to_json())
            .with_context(|| format!("保存冒险状态失败: {}", path.display()))
    }

    /// 执行一个回合：规则判定（确定性）→ 状态更新 → 叙述（注入）。
    pub fn take_turn(&mut self, action: &str, narrator: &dyn Narrator) -> TurnResult {
        self.turn += 1;
        let outcome = self.resolve(action);
        self.apply(&outcome);
        let narration = narrator.narrate(self, action, &outcome);
        TurnResult {
            turn: self.turn,
            action: action.to_string(),
            outcome,
            narration,
            state_after: self.clone(),
        }
    }

    /// 确定性规则解析：不调用任何 LLM。
    fn resolve(&mut self, action: &str) -> ActionOutcome {
        let lower = action.to_lowercase();
        if lower.starts_with("攻击") || lower.starts_with("attack") {
            let Some(enemy) = self.enemy.clone() else {
                return ActionOutcome::NoTarget;
            };
            let roll = self.rng.roll(20);
            let hit = roll >= 10;
            let damage = if hit {
                (self.character.attack + self.rng.roll(6) as i32).max(1)
            } else {
                0
            };
            ActionOutcome::Attack {
                enemy: enemy.name,
                roll,
                hit,
                damage,
            }
        } else if lower.starts_with("遭遇") || lower.starts_with("encounter") {
            let mut parts = action.split_whitespace();
            let raw = parts.nth(1).unwrap_or("敌人");
            // 尾随数字视为初始 HP（如「遭遇 巨魔30」）。
            let (name, hp) = match raw.find(|c: char| c.is_ascii_digit()) {
                Some(i) => {
                    let name = raw[..i].to_string();
                    let hp = raw[i..].parse::<i32>().unwrap_or(20);
                    (name, hp.max(1))
                }
                None => (raw.to_string(), 20),
            };
            ActionOutcome::Encounter { enemy: name, hp }
        } else if lower.starts_with("前往") || lower.starts_with("goto ") {
            let to = action
                .split_whitespace()
                .nth(1)
                .unwrap_or("未知之地")
                .to_string();
            ActionOutcome::Move { to }
        } else if lower.starts_with("拾取") || lower.starts_with("take ") {
            let item = action
                .split_whitespace()
                .nth(1)
                .unwrap_or("某物")
                .to_string();
            ActionOutcome::Item { item }
        } else {
            ActionOutcome::Unknown
        }
    }

    /// 把结果应用到状态（确定性）。
    fn apply(&mut self, outcome: &ActionOutcome) {
        match outcome {
            ActionOutcome::Attack { enemy, damage, .. } => {
                if let Some(current) = self.enemy.as_mut() {
                    if current.name == *enemy {
                        current.hp = (current.hp - damage).max(0);
                        if current.hp == 0 {
                            self.enemy = None;
                        }
                    }
                }
            }
            ActionOutcome::Encounter { enemy, hp } => {
                self.enemy = Some(Enemy {
                    name: enemy.clone(),
                    hp: *hp,
                    max_hp: *hp,
                });
            }
            ActionOutcome::Move { to } => {
                self.scene.clone_from(to);
            }
            ActionOutcome::Item { item } => {
                if !self.inventory.contains(item) {
                    self.inventory.push(item.clone());
                }
            }
            ActionOutcome::NoTarget | ActionOutcome::Unknown => {}
        }
    }
}

/// 一个回合的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    pub turn: u32,
    pub action: String,
    pub outcome: ActionOutcome,
    pub narration: String,
    pub state_after: AdventureState,
}

/// 测试/离线用假叙述器：输出确定性文本。
#[derive(Debug, Default)]
pub struct StubNarrator;

impl Narrator for StubNarrator {
    fn narrate(&self, state: &AdventureState, action: &str, outcome: &ActionOutcome) -> String {
        match outcome {
            ActionOutcome::Attack {
                enemy,
                roll,
                hit,
                damage,
                ..
            } => {
                let left = state
                    .enemy
                    .as_ref()
                    .map(|e| format!("{} HP 剩 {}。", e.name, e.hp))
                    .unwrap_or_else(|| format!("{enemy} 倒下了。"));
                format!(
                    "{} 攻击 {enemy}（骰{roll}）：{}，伤害 {damage}。{left}",
                    state.character.name,
                    if *hit { "命中" } else { "未命中" }
                )
            }
            ActionOutcome::Encounter { enemy, hp } => {
                format!("{} 出现了（HP {hp}）！", enemy)
            }
            ActionOutcome::NoTarget => "没有可攻击的目标。".to_string(),
            ActionOutcome::Move { to } => format!("{} 来到 {to}。", state.character.name),
            ActionOutcome::Item { item } => {
                format!("{} 拾取了 {item}。", state.character.name)
            }
            ActionOutcome::Unknown => format!("{} {action}。", state.character.name),
        }
    }
}

/// 默认叙述模型端点（生成网关，同 `tools::judge` 约定）。
const DEFAULT_NARRATOR_BASE_URL: &str = "http://127.0.0.1:3211/v1";

/// 构造叙述 prompt（纯函数，可测试）：规则与状态给足，LLM 只负责叙述。
#[must_use]
pub fn narrator_prompt(
    state: &AdventureState,
    action: &str,
    outcome: &ActionOutcome,
) -> (String, String) {
    let mut system = format!(
        "你是文字冒险游戏的主持人。游戏「{}」。\
玩家角色：{}（HP {}/{}，攻击 {}，防御 {}）。当前场景：{}。",
        state.world,
        state.character.name,
        state.character.hp,
        state.character.max_hp,
        state.character.attack,
        state.character.defense,
        state.scene
    );
    if let Some(enemy) = &state.enemy {
        system.push_str(&format!(
            "\n当前敌人：{}（HP {}/{}）。",
            enemy.name, enemy.hp, enemy.max_hp
        ));
    }
    system.push_str(
        "\n规则判定由系统已经完成，你只负责把结果叙述得生动：第二人称、\
感官细节、克制形容词；不得替玩家做决定，不得改变任何数值。",
    );
    if !state.lore.is_empty() {
        system.push_str(&format!("\n\n相关设定：\n{}", state.lore));
    }
    let facts = match outcome {
        ActionOutcome::Attack {
            enemy,
            roll,
            hit,
            damage,
        } => {
            let after = if *damage > 0 && state.enemy.is_none() {
                format!("{enemy} 被击倒。")
            } else {
                String::new()
            };
            format!(
                "对 {enemy} 骰点 {roll}/20，{}，伤害 {damage}。{after}",
                if *hit { "命中" } else { "未命中" }
            )
        }
        ActionOutcome::Encounter { enemy, hp } => {
            format!("遭遇 {enemy}（HP {hp}）。")
        }
        ActionOutcome::NoTarget => "没有可攻击的目标。".to_string(),
        ActionOutcome::Move { to } => format!("移动到 {to}。"),
        ActionOutcome::Item { item } => format!("获得物品 {item}。"),
        ActionOutcome::Unknown => "系统无法归类的自由行动。".to_string(),
    };
    let user = format!("玩家行动：{action}\n系统判定结果：{facts}\n请叙述这一回合。");
    (system, user)
}

/// LLM 叙述器：调用生成模型（生成网关）叙述回合。
///
/// 端点不可用时降级到 `StubNarrator`，并通过 [`LlmNarrator::fell_back`]
/// 暴露是否发生了降级（CLI 据此提示用户）。
#[derive(Debug, Clone)]
pub struct LlmNarrator {
    base_url: String,
    api_key: String,
    model: String,
    fell_back: std::cell::Cell<bool>,
}

impl LlmNarrator {
    /// 由端点参数创建；`api_key` 为空时仍可尝试本地端点。
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            fell_back: std::cell::Cell::new(false),
        }
    }

    /// 从环境变量构造：`GATEWAY_BASE_URL` / `GATEWAY_API_KEY` / `GATEWAY_MODEL`。
    #[must_use]
    pub fn from_env() -> Self {
        let base_url = std::env::var("GATEWAY_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_NARRATOR_BASE_URL.to_string());
        let api_key = std::env::var("GATEWAY_API_KEY").unwrap_or_default();
        let model =
            std::env::var("GATEWAY_MODEL").unwrap_or_else(|_| "agnes-3.0-flash".to_string());
        Self::new(base_url, api_key, model)
    }

    /// 是否发生过端点失败降级。
    #[must_use]
    pub fn fell_back(&self) -> bool {
        self.fell_back.get()
    }
}

impl Narrator for LlmNarrator {
    fn narrate(&self, state: &AdventureState, action: &str, outcome: &ActionOutcome) -> String {
        let (system, user) = narrator_prompt(state, action, outcome);
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ],
            "temperature": 0.7,
            "max_tokens": 512,
        });
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut request = ureq::post(&url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json");
        if !self.api_key.is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", self.api_key));
        }
        match request.send_string(&body.to_string()) {
            Ok(resp) => match resp.into_string() {
                Ok(text) => match extract_narration(&text) {
                    Some(n) => n,
                    None => {
                        self.fell_back.set(true);
                        StubNarrator.narrate(state, action, outcome)
                    }
                },
                Err(_) => {
                    self.fell_back.set(true);
                    StubNarrator.narrate(state, action, outcome)
                }
            },
            Err(_) => {
                self.fell_back.set(true);
                StubNarrator.narrate(state, action, outcome)
            }
        }
    }
}

/// 从 chat/completions 响应 JSON 提取叙述文本（纯函数，可测试）。
#[must_use]
pub fn extract_narration(response_body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(response_body).ok()?;
    let content = value
        .pointer("/choices/0/message/content")?
        .as_str()?
        .trim();
    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}
