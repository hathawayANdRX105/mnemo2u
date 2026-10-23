//! style CLI：风格画像与设定卡的查看/校验子命令。
//!
//! - `style-inspect <ID>`：打印注入 prompt 用的风格画像；
//! - `style-check <目录>`：加载并 lint 风格/设定卡配置（解析、id 唯一性）。

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::style::{duplicate_free, render_cards, select_cards, SettingCardSet, StyleProfileSet};

/// `style-inspect`：打印某风格 prompt；`--scene` 附带该场景命中的设定卡。
pub fn inspect(
    styles_dir: &std::path::Path,
    cards_dir: Option<&std::path::Path>,
    id: &str,
    scene: Option<&str>,
) -> Result<i32> {
    let mut found = None;
    for path in collect_yaml(styles_dir)? {
        let set = StyleProfileSet::load(&path)
            .with_context(|| format!("加载风格配置 {}", path.display()))?;
        if set.find(id).is_some() {
            found = Some(style_prompt_found(&set, id, path));
            break;
        }
    }
    let Some((prompt, _)) = found else {
        bail!("未找到风格 {id}（扫描 {}）", styles_dir.display());
    };
    print!("{prompt}");
    if let (Some(cards_dir), Some(scene)) = (cards_dir, scene) {
        for path in collect_yaml(cards_dir)? {
            let set = SettingCardSet::load(&path)
                .with_context(|| format!("加载设定卡 {}", path.display()))?;
            let hit = select_cards(&set, scene);
            if !hit.is_empty() {
                print!("\n{}", render_cards(&hit));
            }
        }
    }
    Ok(0)
}

fn style_prompt_found(
    set: &StyleProfileSet,
    id: &str,
    path: std::path::PathBuf,
) -> (String, PathBuf) {
    let prompt = set.render_prompt(id).unwrap_or_default();
    (prompt, path)
}

/// `style-check`：加载目录下全部 yaml，返回发现的问题列表（空 = 通过）。
pub fn check(styles_dir: &std::path::Path, cards_dir: &std::path::Path) -> Result<i32> {
    let mut problems: Vec<String> = Vec::new();
    for path in collect_yaml(styles_dir)? {
        match StyleProfileSet::load(&path) {
            Ok(set) => {
                let ids: Vec<&str> = set.styles.iter().map(|s| s.id.as_str()).collect();
                let mut sorted = ids.clone();
                sorted.sort_unstable();
                sorted.dedup();
                if sorted.len() != ids.len() {
                    problems.push(format!("{}: 风格 id 重复", path.display()));
                }
            }
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    for path in collect_yaml(cards_dir)? {
        match SettingCardSet::load(&path) {
            Ok(set) => {
                if !duplicate_free(&set) {
                    problems.push(format!(
                        "{}: 设定卡 id 重复: {:?}",
                        path.display(),
                        set.duplicate_ids()
                    ));
                }
            }
            Err(e) => problems.push(format!("{}: {e}", path.display())),
        }
    }
    if problems.is_empty() {
        println!("style-check: OK");
        Ok(0)
    } else {
        for p in &problems {
            eprintln!("style-check: {p}");
        }
        Ok(2)
    }
}

/// 收集目录（或单文件）下的全部 yaml，按路径排序。
fn collect_yaml(dir: &std::path::Path) -> Result<Vec<PathBuf>> {
    if dir.is_file() {
        return Ok(vec![dir.to_path_buf()]);
    }
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("读取目录失败: {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// `adventure` 回合命令选项。
#[derive(Debug, Clone)]
pub struct AdventureOptions {
    /// 状态文件路径（不存在且 `new_world` 时创建）。
    pub state: std::path::PathBuf,
    /// 玩家行动文本。
    pub action: String,
    /// 新建世界（覆盖已有状态文件）。
    pub new_world: bool,
    /// 世界名。
    pub world: String,
    /// 起始场景。
    pub scene: String,
    /// 角色名。
    pub name: String,
    /// HP / 攻击 / 防御（新世界时）。
    pub hp: i32,
    pub attack: i32,
    pub defense: i32,
    /// 使用 LLM 叙述器（否则确定性桩叙述器）。
    pub llm: bool,
    /// 设定卡目录（动态注入当前场景相关设定）。
    pub cards_dir: Option<std::path::PathBuf>,
    /// 用于匹配设定卡的场景文本（默认取行动文本）。
    pub scene_text: Option<String>,
}

/// 执行一个冒险回合；返回进程退出码。
pub fn adventure(opts: &AdventureOptions) -> Result<i32> {
    use crate::style::adventure::{AdventureState, Character, LlmNarrator, Narrator, StubNarrator};

    let mut state = if opts.new_world || !opts.state.exists() {
        AdventureState::new(
            opts.world.clone(),
            opts.scene.clone(),
            Character {
                name: opts.name.clone(),
                hp: opts.hp,
                max_hp: opts.hp.max(1),
                attack: opts.attack,
                defense: opts.defense,
            },
        )
    } else {
        AdventureState::load(&opts.state)?
    };

    // 设定卡动态注入：按场景文本命中关键词，渲染进 lore 并持久化。
    if let Some(cards_dir) = &opts.cards_dir {
        let scene_text = opts.scene_text.as_deref().unwrap_or(&opts.action);
        let mut lore = String::new();
        for path in collect_yaml(cards_dir)? {
            let set = SettingCardSet::load(&path)
                .with_context(|| format!("加载设定卡 {}", path.display()))?;
            let hit = select_cards(&set, scene_text);
            if !hit.is_empty() {
                lore.push_str(&render_cards(&hit));
            }
        }
        state.lore = lore;
    }

    let narration = if opts.llm {
        let narrator = LlmNarrator::from_env();
        let result = state.take_turn(&opts.action, &narrator);
        if narrator.fell_back() {
            eprintln!("注意：LLM 端点不可用，已降级为确定性叙述。");
        }
        result
    } else {
        let narrator: &dyn Narrator = &StubNarrator;
        state.take_turn(&opts.action, narrator)
    };

    state.save(&opts.state)?;

    println!("--- 回合 {} ---", narration.turn);
    println!("{}", narration.narration);
    println!(
        "[状态] 场景: {} | HP: {}/{} | 敌人: {} | 物品: {}",
        state.scene,
        state.character.hp,
        state.character.max_hp,
        state
            .enemy
            .as_ref()
            .map(|e| format!("{}（HP {}）", e.name, e.hp))
            .unwrap_or_else(|| "无".to_string()),
        if state.inventory.is_empty() {
            "无".to_string()
        } else {
            state.inventory.join("、")
        }
    );
    println!("[存档] {}", opts.state.display());
    Ok(0)
}
