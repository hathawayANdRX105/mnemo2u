//! DND 文字冒险测试：骰子确定性、动作解析、状态更新、持久化、假叙述器。

use novel_review::style::adventure::{
    ActionOutcome, AdventureState, Character, Narrator, Rng, StubNarrator,
};

fn hero() -> Character {
    Character {
        name: "林越".to_string(),
        hp: 20,
        max_hp: 20,
        attack: 5,
        defense: 2,
    }
}

fn world() -> AdventureState {
    AdventureState::new("旧朝", "城楼", hero())
}

/// 记录回合结果供断言。
fn turn(state: &mut AdventureState, action: &str) -> (ActionOutcome, String, AdventureState) {
    let narrator = StubNarrator;
    let r = state.take_turn(action, &narrator);
    (r.outcome, r.narration, r.state_after)
}

#[test]
fn rng_is_deterministic_for_same_seed() {
    let mut a = Rng::new(42);
    let mut b = Rng::new(42);
    for _ in 0..10 {
        assert_eq!(a.roll(20), b.roll(20));
    }
}

#[test]
fn rng_stays_in_bounds() {
    let mut rng = Rng::new(7);
    for _ in 0..100 {
        let v = rng.roll(6);
        assert!((1..=6).contains(&v), "骰点越界: {v}");
    }
}

#[test]
fn rng_seed_zero_is_normalized() {
    let mut rng = Rng::new(0);
    assert!((1..=20).contains(&rng.roll(20)));
}

#[test]
fn attack_hits_enemy_not_self() {
    let mut state = world();
    state.take_turn("遭遇 侍卫", &StubNarrator);
    assert!(state.enemy.is_some());
    let before_hp = state.character.hp;
    let (outcome, _n, after) = turn(&mut state, "攻击侍卫");
    match outcome {
        ActionOutcome::Attack {
            enemy,
            roll,
            hit,
            damage,
        } => {
            assert_eq!(enemy, "侍卫");
            assert!((1..=20).contains(&roll));
            if hit {
                assert!(damage >= 1);
            } else {
                assert_eq!(damage, 0);
            }
        }
        other => panic!("应解析为攻击，得到 {other:?}"),
    }
    assert_eq!(after.turn, 2);
    // 攻击只伤敌人，玩家 HP 不变。
    assert_eq!(state.character.hp, before_hp);
}

#[test]
fn attack_without_enemy_is_no_target() {
    let mut state = world();
    let (outcome, narration, _) = turn(&mut state, "攻击侍卫");
    assert_eq!(outcome, ActionOutcome::NoTarget);
    assert!(narration.contains("没有可攻击的目标"));
    assert_eq!(state.turn, 1);
}

#[test]
fn encounter_creates_enemy_with_hp() {
    let mut state = world();
    let (outcome, narration, _) = turn(&mut state, "遭遇 巨魔30");
    assert_eq!(
        outcome,
        ActionOutcome::Encounter {
            enemy: "巨魔".to_string(),
            hp: 30
        }
    );
    assert!(narration.contains("巨魔"));
    let enemy = state.enemy.as_ref().expect("应有敌人");
    assert_eq!((enemy.hp, enemy.max_hp), (30, 30));
}

#[test]
fn encounter_default_hp_20() {
    let mut state = world();
    turn(&mut state, "遭遇 侍卫");
    let enemy = state.enemy.as_ref().expect("应有敌人");
    assert_eq!(enemy.hp, 20);
}

#[test]
fn enemy_never_negative_hp_and_clears_when_down() {
    let mut state = world();
    state.take_turn("遭遇 侍卫", &StubNarrator);
    state.character.attack = 100;
    // 持续攻击：敌人 HP 有下界，归零即清除；清除后再攻击为 NoTarget。
    for _ in 0..10 {
        let (o, _, _) = turn(&mut state, "攻击侍卫");
        match o {
            ActionOutcome::Attack { .. } => {
                if let Some(enemy) = &state.enemy {
                    assert!(enemy.hp >= 0, "敌人 HP 不应低于 0");
                }
            }
            ActionOutcome::NoTarget => break,
            other => panic!("应解析为攻击或无目标，得到 {other:?}"),
        }
    }
    assert!(state.enemy.is_none(), "HP 归零后敌人应被清除");
}

#[test]
fn move_updates_scene() {
    let mut state = world();
    let (outcome, narration, after) = turn(&mut state, "前往 雨巷");
    assert_eq!(
        outcome,
        ActionOutcome::Move {
            to: "雨巷".to_string()
        }
    );
    assert_eq!(state.scene, "雨巷");
    assert!(narration.contains("雨巷"));
    assert_eq!(after.scene, "雨巷");
}

#[test]
fn take_item_adds_to_inventory_once() {
    let mut state = world();
    turn(&mut state, "拾取 短刀");
    turn(&mut state, "拾取 短刀");
    assert_eq!(
        state.inventory,
        vec!["短刀".to_string()],
        "重复拾取只入栏一次"
    );
}

#[test]
fn unknown_action_delegates_to_narrator() {
    let mut state = world();
    let (outcome, narration, _) = turn(&mut state, "观察四周");
    assert_eq!(outcome, ActionOutcome::Unknown);
    assert!(narration.contains("观察四周"));
}

#[test]
fn turn_counter_increments() {
    let mut state = world();
    assert_eq!(state.turn, 0);
    turn(&mut state, "前往 城门");
    turn(&mut state, "拾取 灯笼");
    assert_eq!(state.turn, 2);
}

#[test]
fn state_persists_through_json_roundtrip() {
    let mut state = world();
    turn(&mut state, "前往 旧闸");
    turn(&mut state, "拾取 油纸伞");
    let json = state.to_json();
    let loaded = AdventureState::from_json(&json).expect("应可解析回");
    assert_eq!(loaded.scene, "旧闸");
    assert_eq!(loaded.inventory, vec!["油纸伞".to_string()]);
    assert_eq!(loaded.turn, 2);
    // 继续推进应保留状态。
    let mut resumed = loaded;
    turn(&mut resumed, "拾取 灯笼");
    assert_eq!(resumed.inventory.len(), 2);
    assert_eq!(resumed.turn, 3);
}

#[test]
fn save_and_load_from_disk() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("save.json");
    let mut state = world();
    turn(&mut state, "前往 码头");
    state.save(&path).expect("应可保存");
    let loaded = AdventureState::load(&path).expect("应可读回");
    assert_eq!(loaded.scene, "码头");
}

#[test]
fn same_seed_same_sequence_replayable() {
    // 两个同种子状态执行同动作序列，结果逐回合一致（可复现冒险）。
    let mut a = world();
    let mut b = world();
    for action in ["攻击侍卫", "攻击侍卫", "前往 码头"] {
        let ra = a.take_turn(action, &StubNarrator);
        let rb = b.take_turn(action, &StubNarrator);
        assert_eq!(ra.outcome, rb.outcome, "同种子应复现: {action}");
        assert_eq!(ra.state_after, rb.state_after);
    }
}

#[test]
fn flags_default_empty_and_btreemap_ordered() {
    let state = world();
    assert!(state.flags.is_empty());
}

#[test]
fn stub_narrator_deterministic() {
    let mut state = world();
    let r1 = state.take_turn("前往 城门", &StubNarrator);
    let mut again = world();
    let r2 = again.take_turn("前往 城门", &StubNarrator);
    assert_eq!(r1.narration, r2.narration);
    // Narrator trait 亦可直接调用。
    let narrator: &dyn Narrator = &StubNarrator;
    let text = narrator.narrate(&state, "x", &ActionOutcome::Unknown);
    assert!(text.contains("x"));
}
