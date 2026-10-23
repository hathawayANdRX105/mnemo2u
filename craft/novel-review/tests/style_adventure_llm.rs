//! LLM 叙述器测试：prompt 构造、响应解析、端点失败降级、lore 注入。
//!
//! 降级测试指向一个必然拒绝连接的端口（127.0.0.1:1），无需真实端点。

use novel_review::style::adventure::{
    extract_narration, narrator_prompt, ActionOutcome, AdventureState, Character, LlmNarrator,
    Narrator, StubNarrator,
};

fn world() -> AdventureState {
    AdventureState::new(
        "旧朝风云",
        "城楼",
        Character {
            name: "林越".to_string(),
            hp: 20,
            max_hp: 20,
            attack: 5,
            defense: 2,
        },
    )
}

#[test]
fn prompt_contains_state_and_facts() {
    let state = world();
    let (system, user) = narrator_prompt(
        &state,
        "攻击侍卫",
        &ActionOutcome::Attack {
            enemy: "侍卫".to_string(),
            roll: 15,
            hit: true,
            damage: 8,
        },
    );
    assert!(system.contains("旧朝风云"));
    assert!(system.contains("林越"));
    assert!(system.contains("HP 20/20"));
    assert!(system.contains("第二人称"));
    assert!(system.contains("不得替玩家做决定"));
    assert!(user.contains("攻击侍卫"));
    assert!(user.contains("对 侍卫 骰点 15/20"));
    assert!(user.contains("命中"));
    assert!(user.contains("伤害 8"));
}

#[test]
fn prompt_includes_enemy_state() {
    let mut state = world();
    state.enemy = Some(novel_review::style::adventure::Enemy {
        name: "巨魔".to_string(),
        hp: 12,
        max_hp: 30,
    });
    let (system, _) = narrator_prompt(&state, "攻击巨魔", &ActionOutcome::NoTarget);
    assert!(system.contains("当前敌人：巨魔（HP 12/30）"));
}

#[test]
fn prompt_move_and_item_outcomes() {
    let state = world();
    let (_, user_m) = narrator_prompt(
        &state,
        "前往 雨巷",
        &ActionOutcome::Move {
            to: "雨巷".into()
        },
    );
    assert!(user_m.contains("移动到 雨巷"));
    let (_, user_i) = narrator_prompt(
        &state,
        "拾取 油纸伞",
        &ActionOutcome::Item {
            item: "油纸伞".into(),
        },
    );
    assert!(user_i.contains("获得物品 油纸伞"));
    let (_, user_u) = narrator_prompt(&state, "观察四周", &ActionOutcome::Unknown);
    assert!(user_u.contains("系统无法归类"));
}

#[test]
fn lore_injected_into_system_prompt() {
    let mut state = world();
    state.lore = "# 相关设定\n\n## lin-yue\n林越：前朝余孽。".to_string();
    let (system, _) = narrator_prompt(&state, "攻击", &ActionOutcome::Unknown);
    assert!(system.contains("相关设定"));
    assert!(system.contains("前朝余孽"));
}

#[test]
fn extract_narration_valid() {
    let body = r#"{"choices":[{"message":{"content":"  你拔出了短刀。  "}}]}"#;
    assert_eq!(extract_narration(body).as_deref(), Some("你拔出了短刀。"));
}

#[test]
fn extract_narration_invalid_inputs() {
    // 空 content
    assert!(extract_narration(r#"{"choices":[{"message":{"content":"  "}}]}"#).is_none());
    // 缺字段
    assert!(extract_narration(r#"{"choices":[{"message":{}}]}"#).is_none());
    assert!(extract_narration(r#"{"choices":[]}"#).is_none());
    // 非 JSON
    assert!(extract_narration("not json").is_none());
}

#[test]
fn llm_narrator_falls_back_when_endpoint_dead() {
    // 端口 1 必然拒绝连接：验证降级到确定性叙述且标记可见。
    let narrator = LlmNarrator::new("http://127.0.0.1:1/v1", "", "test-model");
    let mut state = world();
    let result = state.take_turn("前往 城门", &narrator);
    let stub = StubNarrator.narrate(&state, "前往 城门", &result.outcome);
    assert_eq!(result.narration, stub, "降级应产出桩叙述");
    assert!(narrator.fell_back(), "应标记发生降级");
}

#[test]
fn llm_narrator_fallback_advances_state_normally() {
    // 降级不影响规则状态推进（规则与叙述分离）。
    let narrator = LlmNarrator::new("http://127.0.0.1:1/v1", "", "test-model");
    let mut state = world();
    state.take_turn("拾取 短刀", &narrator);
    assert_eq!(state.turn, 1);
    assert_eq!(state.inventory, vec!["短刀".to_string()]);
}
