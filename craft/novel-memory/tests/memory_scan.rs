//! 记忆层：story 时间坐标、槽位键、内容哈希、语料扫描。
//!
//! 这些测试**不碰嵌入模型**——`corpus::scan` 是纯文件扫描，嵌入只在写库时
//! 才需要（那一步要走本地 ONNX，测试里不跑）。

use std::fs;
use std::path::PathBuf;

use novel_memory::config::{Config, Paths};
use novel_memory::corpus;
use novel_memory::hash;
use novel_memory::storytime::{self, Layer};

fn memory_config() -> Config {
    Config::default()
}

fn paths_config() -> Paths {
    Paths::default()
}

/// 临时目录里的最小小说树。
///
/// 布局按 `PathsConfig` 的默认名：概念卡在 `concept/cards/characters/`、
/// 正文在 `drafts/<story>/`、施工图在 `chapter-plan/<story>/`。
struct Novel {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Novel {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let novel = Self { _dir: dir, root };
        novel.write(
            "concept/cards/characters/lilian.md",
            "# 莉莉安·格雷普\n\n\
             - 卡片 ID：CHR-LIL-01\n\
             - 状态：已确认\n\
             - 所属分类：角色\n\
             - 别名 / 英文名：Lilian Grape\n\
             - 关联卡片：`characters/red.md`\n\n\
             ## 身份\n\n\
             - 身份：蓝渊制造大小姐\n\
             - 阵营：主角团\n\
             - 社会位置：高位出身\n\n\
             ## 关系\n\n\
             - 对雷德：搭档\n\
             - 对 Pi：雇用\n",
        );
        novel.write(
            "concept/cards/characters/red.md",
            "# 雷德·纳西索斯\n\n\
             - 卡片 ID：CHR-RED-01\n\
             - 状态：已确认\n\
             - 所属分类：角色\n\
             - 别名 / 英文名：Red Narcissus\n\
             - 关联卡片：\n\n\
             ## 身份\n\n\
             - 身份：学院学生\n\
             - 阵营：主角团\n\
             - 社会位置：底层出身\n",
        );
        novel.write(
            "drafts/story-1-ambush/ch01.md",
            "# 第一章\n\n\
             莉莉安把枪带从雷德肩侧拽开。\n\n\
             雷德没抬头。\n",
        );
        novel.write(
            "drafts/story-1-ambush/ch02.md",
            "# 第二章\n\n\
             雷德看着终端上的警告。\n",
        );
        // 单元级时间标注放在目录的 README.md 里（corpus 只从那里读标记）。
        novel.write(
            "drafts/interlude1/README.md",
            "# 插叙单元\n\n\
             <!-- story-time: 0.5 -->\n",
        );
        novel.write(
            "drafts/interlude1/ch01.md",
            "# 插叙：更早的事\n\n\
             那时候他们还不认识。\n",
        );
        novel.write(
            "chapter-plan/story-1-ambush/ch01.md",
            "# ch01 施工图\n\n\
             ## 本章功能\n\n\
             - 冲突：莉莉安与雷德的对峙\n\n\
             ## Scene 1：枪带\n\n\
             - 目标：立住两人的搭档关系\n\
             - 看点：\n\
               - 莉莉安拽开枪带\n",
        );
        novel
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().expect("父目录")).expect("建目录");
        fs::write(path, content).expect("写文件");
    }
}

#[test]
fn story_names_map_to_ticks() {
    // story 1 → tick 1000；interlude 1 → 插在 story 1 之后的半拍。
    let (identity, tick) = storytime::parse_unit_name("story-1-ambush").expect("story 单元");
    assert_eq!(identity, "story-1");
    assert_eq!(tick, 1000);

    let (identity, tick) = storytime::parse_unit_name("story-5a-father").expect("story 5a");
    assert_eq!(identity, "story-5a");
    assert_eq!(tick, 5000);

    let (identity, tick) = storytime::parse_unit_name("interlude1").expect("interlude");
    assert_eq!(identity, "interlude-1");
    assert_eq!(tick, 1500);

    assert!(
        storytime::parse_unit_name("chapters").is_none(),
        "不是 story 单元的名字要认出来"
    );
}

#[test]
fn ticks_sort_by_story_time_not_by_name() {
    // 字典序不是故事序：story-10 在 story-2 后面，但故事时间在前。
    let mut units = [
        ("story-10", 10_000),
        ("story-2", 2_000),
        ("interlude-1", 1_500),
    ];
    units.sort_by_key(|(_, tick)| *tick);
    let order: Vec<&str> = units.iter().map(|(name, _)| *name).collect();
    assert_eq!(order, vec!["interlude-1", "story-2", "story-10"]);
}

#[test]
fn task_ids_are_zero_padded_so_lexicographic_order_is_story_order() {
    let (identity, tick) = storytime::parse_unit_name("story-2").expect("story 2");
    let unit = novel_memory::StoryUnit {
        identity,
        tick,
        beat: 0,
        display: "story-2".to_string(),
    };
    assert_eq!(unit.task_id(), "story:002");
    // story 10 的 task_id 必须排在 story 2 之后。
    let (identity, tick) = storytime::parse_unit_name("story-10").expect("story 10");
    let unit = novel_memory::StoryUnit {
        identity,
        tick,
        beat: 0,
        display: "story-10".to_string(),
    };
    assert!(unit.task_id().as_str() > "story:002");
}

#[test]
fn explicit_markers_override_the_unit_tick() {
    let marker = storytime::parse_marker("<!-- story-time: 1.5 beat: 4 -->").expect("标记");
    assert_eq!(marker.tick, 1500);
    assert_eq!(marker.beat, Some(4));

    let plain = storytime::parse_marker("<!-- story-time: 2 -->").expect("标记");
    assert_eq!(plain.tick, 2000);
    assert_eq!(plain.beat, None);

    assert!(storytime::parse_marker("<!-- 别的注释 -->").is_none());
}

#[test]
fn ticks_parse_without_floating_point() {
    assert_eq!(storytime::parse_tick("1"), Some(1000));
    assert_eq!(storytime::parse_tick("1.5"), Some(1500));
    // 右侧补零：`5` 是 0.5 拍，不是 0.005 拍。
    assert_eq!(storytime::parse_tick("1.5"), Some(1500));
    assert_eq!(storytime::parse_tick("2.25"), Some(2250));
    assert_eq!(storytime::parse_tick("abc"), None);
    assert_eq!(storytime::parse_tick("1.1234"), None);
}

#[test]
fn chapter_beats_come_from_the_file_name() {
    assert_eq!(storytime::chapter_beat("ch07.md"), Some(7));
    assert_eq!(storytime::chapter_beat("ch01-信号.md"), None);
    assert_eq!(storytime::chapter_beat("notes.md"), None);
}

#[test]
fn content_hash_is_stable_and_event_ids_carry_it() {
    let text = "雷德把枪带从她肩侧拽开。";
    let first = hash::content_hash(text);
    assert_eq!(first, hash::content_hash(text));
    assert_ne!(first, hash::content_hash("雷德把枪带从她肩侧拽开了。"));

    let event = hash::event_id("draft:story-1:ch01:p0001", text);
    assert!(event.starts_with("draft:story-1:ch01:p0001#"));
    assert_eq!(hash::hash_of_event_id(&event), Some(first.as_str()));
    assert_eq!(hash::hash_of_event_id("some-other-event"), None);
}

#[test]
fn scan_finds_cards_drafts_and_plans() {
    let novel = Novel::new();
    let scanned = corpus::scan(&novel.root, &paths_config(), &memory_config());

    // 概念卡：每张卡一条卡级事实。
    let card_slots: Vec<&novel_memory::FactSlot> = scanned
        .slots
        .iter()
        .filter(|slot| slot.layer == Layer::Setting)
        .collect();
    assert!(
        card_slots.len() >= 2,
        "两张卡各至少一条事实，实际 {}",
        card_slots.len()
    );
    assert!(
        card_slots.iter().any(|slot| slot.key.contains("lilian")),
        "莉莉安的卡要有事实：{:?}",
        card_slots.iter().map(|s| &s.key).collect::<Vec<_>>()
    );

    // 正文：每段一条事实。
    let draft_slots: Vec<&novel_memory::FactSlot> = scanned
        .slots
        .iter()
        .filter(|slot| slot.layer == Layer::Fact)
        .collect();
    assert!(
        !draft_slots.is_empty(),
        "正文要产出事实槽位，扫到 {} 条",
        draft_slots.len()
    );
    assert!(
        draft_slots.iter().any(|slot| slot.key.contains("story-1")),
        "正文槽位要带 story 身份：{:?}",
        draft_slots.iter().map(|s| &s.key).collect::<Vec<_>>()
    );

    // story 单元：从正文目录名解析。
    let identities: Vec<&str> = scanned
        .units
        .iter()
        .map(|unit| unit.identity.as_str())
        .collect();
    assert!(identities.contains(&"story-1"), "{identities:?}");
    assert!(identities.contains(&"interlude-1"), "{identities:?}");
}

#[test]
fn relationship_edges_come_from_card_relations() {
    let novel = Novel::new();
    let scanned = corpus::scan(&novel.root, &paths_config(), &memory_config());

    // 莉莉安的卡声明了「关联卡片：characters/red.md」和「## 关系」小节。
    assert!(
        !scanned.edges.is_empty(),
        "卡片的关联与关系小节要产出边，实际 {} 条",
        scanned.edges.len()
    );
    assert!(
        scanned.edges.iter().any(|edge| edge.src.contains("lilian")),
        "边要挂在正确的卡上：{:?}",
        scanned.edges
    );
}

#[test]
fn explicit_story_time_marker_moves_a_fact_earlier() {
    let novel = Novel::new();
    let scanned = corpus::scan(&novel.root, &paths_config(), &memory_config());

    // interlude1 的目录名给出 tick 1500；README 里的标记给出 0.5 拍。
    let interlude = scanned
        .units
        .iter()
        .find(|unit| unit.identity == "interlude-1")
        .expect("interlude 单元");
    assert_eq!(interlude.tick, 500, "显式标记覆盖目录名给出的时间");
    assert_eq!(interlude.beat, 0, "标记只覆盖时间，不覆盖拍位");
}

#[test]
fn scan_is_idempotent() {
    let novel = Novel::new();
    let first = corpus::scan(&novel.root, &paths_config(), &memory_config());
    let second = corpus::scan(&novel.root, &paths_config(), &memory_config());
    assert_eq!(
        first.slots.len(),
        second.slots.len(),
        "两次扫描的槽位数要一样"
    );
    let first_keys: Vec<&str> = first.slots.iter().map(|slot| slot.key.as_str()).collect();
    let second_keys: Vec<&str> = second.slots.iter().map(|slot| slot.key.as_str()).collect();
    assert_eq!(first_keys, second_keys, "槽位键要稳定");
}

#[test]
fn layer_is_decided_by_the_fact_id_prefix() {
    assert_eq!(Layer::of_fact_id("card:characters/lilian"), Layer::Setting);
    assert_eq!(Layer::of_fact_id("rel:card:x:00"), Layer::Relation);
    assert_eq!(Layer::of_fact_id("draft:story-1:ch01:p0001"), Layer::Fact);
}

/// 目录不存在时不能 panic——空目录也要能扫。
#[test]
fn scan_on_missing_directories_is_empty_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scanned = corpus::scan(dir.path(), &paths_config(), &memory_config());
    assert!(scanned.slots.is_empty());
    assert!(scanned.edges.is_empty());
}
