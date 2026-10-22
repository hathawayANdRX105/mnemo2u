//! 语料扫描：把小说目录读成「事实槽位」。
//!
//! 三层各自有一个来源：
//!
//! | 层 | 来源 | 产出 |
//! |---|---|---|
//! | 设定 | 概念卡 | 每张卡一条卡级事实 + 每个小节一条小节事实 |
//! | 事实 | 正文、施工图、story 大纲 | 每段/每条状态变化一条事实 |
//! | 关系 | 概念卡的关系声明 | 关系边 + 声明事实 + 关系演进事实 |
//!
//! 事实槽位（[`FactSlot`]）是**位置**而不是内容：同一个槽位在下次扫描
//! 里文本变了，就是一次修正（走 mnemo2u 的 `supersede` 链）；文本没变，
//! 就是幂等重扫。
//!
//! story 时间由 [`crate::storytime`] 判定：单元名给出 story 序号，
//! 文件里的 `<!-- story-time: … -->` 标记覆盖它。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mnemo2u::core::types::RelKind;

use crate::config::Config;
use crate::storytime::{self, Layer, StoryUnit, Tick};

/// 一个待写入的事实槽位。
///
/// `key` 是事实 id 的槽位部分（不含修订号）；同一次扫描里 key 唯一。
#[derive(Debug, Clone)]
pub struct FactSlot {
    /// 槽位键（`draft:story-1:ch01:p0001`）。
    pub key: String,
    /// 所属层。
    pub layer: Layer,
    /// 事实正文（被向量化的文本）。
    pub text: String,
    /// story 序号（写进 `Fact.valid_from`）；`None` = 全书级。
    pub valid_from: Option<i64>,
    /// 作用域的 task 分量（story 单元）；`None` = 全书级。
    pub task_id: Option<String>,
    /// 证据文件。
    pub source_path: PathBuf,
    /// 证据行号（1 起）。
    pub source_line: usize,
}

/// 一条待写入的关系边。
///
/// 端点是**槽位键**，写入时解析成最终事实 id。
#[derive(Debug, Clone)]
pub struct EdgeSlot {
    /// 起点槽位键。
    pub src: String,
    /// 终点槽位键。
    pub dst: String,
    /// 关系类型。
    pub rel: RelKind,
    /// 权重。
    pub weight: f64,
    /// 证据文件。
    pub source_path: PathBuf,
}

/// 概念卡清单（命令层用来把实体名对回卡片）。
#[derive(Debug, Clone)]
pub struct CardInfo {
    /// 卡片槽位键（`card:characters/lilian-grape`）。
    pub key: String,
    /// 卡片标题。
    pub title: String,
    /// 分类。
    pub category: String,
    /// 卡片 ID。
    pub card_id: String,
    /// 别名（含英文名）。
    pub aliases: Vec<String>,
    /// 文件路径。
    pub path: PathBuf,
}

/// 扫描结果。
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    /// 事实槽位。
    pub slots: Vec<FactSlot>,
    /// 关系边。
    pub edges: Vec<EdgeSlot>,
    /// story 单元（按身份排序）。
    pub units: Vec<StoryUnit>,
    /// 概念卡。
    pub cards: Vec<CardInfo>,
    /// 实体名（卡片标题 + 别名），用于关系归因。
    pub entity_names: Vec<String>,
}

impl Corpus {
    /// 按名字找卡片（精确匹配优先，其次包含匹配）。
    pub fn find_card(&self, name: &str) -> Option<&CardInfo> {
        if let Some(card) = self.cards.iter().find(|card| {
            card.title == name
                || card.card_id == name
                || card.aliases.iter().any(|alias| alias == name)
        }) {
            return Some(card);
        }
        self.cards.iter().find(|card| {
            card.title.contains(name) || card.aliases.iter().any(|alias| alias.contains(name))
        })
    }
}

/// 扫描小说目录，产出三层语料。
pub fn scan(novel_dir: &Path, paths: &crate::config::Paths, memory: &Config) -> Corpus {
    let mut corpus = Corpus::default();
    let mut units: BTreeMap<String, StoryUnit> = BTreeMap::new();

    collect_units(novel_dir, &paths.drafts, &mut units);
    collect_units(novel_dir, paths.chapter_plan.as_str(), &mut units);
    collect_story_plan_units(novel_dir, paths.story_plan.as_str(), &mut units);
    apply_unit_markers(novel_dir, paths, &mut units);

    corpus.units = units.into_values().collect();
    // The collectors need the units *and* the corpus they fill, so the units
    // are read through a separate handle rather than borrowed from the
    // corpus being mutated.
    let units = corpus.units.clone();

    collect_cards(novel_dir, &paths.cards, memory, &mut corpus);
    collect_drafts(novel_dir, &paths.drafts, &units, &mut corpus);
    collect_chapter_plans(novel_dir, &paths.chapter_plan, &units, &mut corpus);
    collect_story_plans(novel_dir, &paths.story_plan, &units, memory, &mut corpus);

    corpus.entity_names = corpus
        .cards
        .iter()
        .flat_map(|card| std::iter::once(card.title.clone()).chain(card.aliases.iter().cloned()))
        .filter(|name| !name.is_empty())
        .collect();
    corpus.entity_names.sort();
    corpus.entity_names.dedup();

    corpus
}

/// 收集目录下的 story 单元（每个子目录一个单元）。
fn collect_units(novel_dir: &Path, dir: &str, units: &mut BTreeMap<String, StoryUnit>) {
    // The directory is resolved against the novel directory, not the
    // process's working directory.
    let Ok(entries) = std::fs::read_dir(novel_dir.join(dir)) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        let Some((identity, tick)) = storytime::parse_unit_name(&name) else {
            continue;
        };
        units.entry(identity.clone()).or_insert(StoryUnit {
            identity,
            tick,
            beat: 0,
            display: name,
        });
    }
}

/// 收集 story 大纲目录里的单元（每个文件一个单元）。
fn collect_story_plan_units(novel_dir: &Path, dir: &str, units: &mut BTreeMap<String, StoryUnit>) {
    let Ok(entries) = std::fs::read_dir(novel_dir.join(dir)) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();
    for name in names {
        let stem = name.trim_end_matches(".md");
        let Some((identity, tick)) = storytime::parse_unit_name(stem) else {
            continue;
        };
        units.entry(identity.clone()).or_insert(StoryUnit {
            identity,
            tick,
            beat: 0,
            display: stem.to_string(),
        });
    }
}

/// 单元级标记：单元目录里的 `README.md` 可以带 `<!-- story-time: … -->`。
///
/// 章节文件自己的标记优先于单元标记，所以一章可以是插叙而单元不是。
fn apply_unit_markers(
    novel_dir: &Path,
    paths: &crate::config::Paths,
    units: &mut BTreeMap<String, StoryUnit>,
) {
    for dir in [&paths.drafts, &paths.chapter_plan] {
        let root = novel_dir.join(dir);
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(display) = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            let Some((identity, _)) = storytime::parse_unit_name(&display) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(path.join("README.md")) else {
                continue;
            };
            let Some(marker) = storytime::parse_marker(&text) else {
                continue;
            };
            if let Some(unit) = units.get_mut(&identity) {
                unit.tick = marker.tick;
            }
        }
    }
}

/// 概念卡 → 设定层事实 + 关系层事实与边。
fn collect_cards(novel_dir: &Path, cards_dir: &str, memory: &Config, corpus: &mut Corpus) {
    let root = novel_dir.join(cards_dir);
    for path in markdown_files_recursive(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let slug = relative.trim_end_matches(".md").to_string();
        let card = parse_card(&text);
        let key = format!("card:{slug}");
        corpus.cards.push(CardInfo {
            key: key.clone(),
            title: card.title.clone(),
            category: card.category.clone(),
            card_id: card.card_id.clone(),
            aliases: card.aliases.clone(),
            path: path.clone(),
        });

        corpus.slots.push(FactSlot {
            key: key.clone(),
            layer: Layer::Setting,
            text: card.header_text(),
            valid_from: None,
            task_id: None,
            source_path: path.clone(),
            source_line: 1,
        });

        for (section_title, body) in &card.sections {
            let body = body.trim();
            if body.is_empty() {
                continue;
            }
            corpus.slots.push(FactSlot {
                key: format!("{key}:{}", slugify(section_title)),
                layer: Layer::Setting,
                text: format!("{} · {section_title}\n{body}", card.title),
                valid_from: None,
                task_id: None,
                source_path: path.clone(),
                source_line: 1,
            });
        }

        collect_card_relations(&card, &key, memory, &path, corpus);
    }
}

/// 概念卡里的关系声明 → 关系事实 + 关系边。
fn collect_card_relations(
    card: &ParsedCard,
    card_key: &str,
    memory: &Config,
    path: &Path,
    corpus: &mut Corpus,
) {
    // 关联卡片字段：卡片与卡片之间的边。
    if let Some(field) = card.fields.get(&memory.card_related_field) {
        for reference in extract_backticked(field) {
            let target = reference.trim_end_matches(".md");
            corpus.edges.push(EdgeSlot {
                src: card_key.to_string(),
                dst: format!("card:{target}"),
                rel: RelKind::Related,
                weight: 1.0,
                source_path: path.to_path_buf(),
            });
        }
    }

    let Some((_, section)) = card
        .sections
        .iter()
        .find(|(name, _)| name == &memory.card_relation_section)
    else {
        return;
    };
    let mut index = 0usize;
    for bullet in section_bullets(section) {
        let Some((target, description)) = split_relation(&bullet) else {
            continue;
        };
        index += 1;
        corpus.slots.push(FactSlot {
            key: format!("rel:{card_key}:{index:02}"),
            layer: Layer::Relation,
            text: format!("{} 与 {} 的关系：{description}", card.title, target),
            valid_from: None,
            task_id: None,
            source_path: path.to_path_buf(),
            source_line: 1,
        });
        corpus.edges.push(EdgeSlot {
            src: card_key.to_string(),
            dst: format!("entity:{target}"),
            rel: RelKind::Related,
            weight: 1.0,
            source_path: path.to_path_buf(),
        });
    }
}

/// 正文 → 事实层事实（每段一条）。
fn collect_drafts(novel_dir: &Path, drafts_dir: &str, units: &[StoryUnit], corpus: &mut Corpus) {
    let root = novel_dir.join(drafts_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let Some(display) = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let Some((identity, default_tick)) = storytime::parse_unit_name(&display) else {
            continue;
        };
        let unit_tick = units
            .iter()
            .find(|unit| unit.identity == identity)
            .map(|unit| unit.tick);
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut chapters: Vec<PathBuf> = files
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && storytime::chapter_beat(&file_name(path)).is_some())
            .collect();
        chapters.sort();
        for chapter in chapters {
            let Ok(text) = std::fs::read_to_string(&chapter) else {
                continue;
            };
            let beat = storytime::chapter_beat(&file_name(&chapter)).unwrap_or(0);
            let marker = storytime::parse_marker(&text);
            let tick = marker
                .map(|marker| marker.tick)
                .or(unit_tick)
                .unwrap_or(default_tick);
            let beat = marker.and_then(|marker| marker.beat).unwrap_or(beat);
            let task_id = task_id_for(tick);
            for (ordinal, paragraph) in paragraphs(&text).into_iter().enumerate() {
                corpus.slots.push(FactSlot {
                    key: format!("draft:{identity}:ch{beat:02}:p{:04}", ordinal + 1),
                    layer: Layer::Fact,
                    text: paragraph,
                    valid_from: Some(story_number(tick)),
                    task_id: task_id.clone(),
                    source_path: chapter.clone(),
                    source_line: 1,
                });
            }
        }
    }
}

/// 章级施工图 → 事实层事实（状态变化）与关系演进事实。
fn collect_chapter_plans(
    novel_dir: &Path,
    plan_dir: &str,
    units: &[StoryUnit],
    corpus: &mut Corpus,
) {
    let root = novel_dir.join(plan_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let Some(display) = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let Some((identity, default_tick)) = storytime::parse_unit_name(&display) else {
            continue;
        };
        let unit_tick = units
            .iter()
            .find(|unit| unit.identity == identity)
            .map(|unit| unit.tick);
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut chapters: Vec<PathBuf> = files
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && storytime::chapter_beat(&file_name(path)).is_some())
            .collect();
        chapters.sort();
        for chapter in chapters {
            let Ok(text) = std::fs::read_to_string(&chapter) else {
                continue;
            };
            let beat = storytime::chapter_beat(&file_name(&chapter)).unwrap_or(0);
            let marker = storytime::parse_marker(&text);
            let tick = marker
                .map(|marker| marker.tick)
                .or(unit_tick)
                .unwrap_or(default_tick);
            let beat = marker.and_then(|marker| marker.beat).unwrap_or(beat);
            let task_id = task_id_for(tick);
            let plan = parse_plan(&text);
            for (scene_index, scene) in plan.scenes.iter().enumerate() {
                for (state_index, statement) in scene.states.iter().enumerate() {
                    let mentions = mentioned_entities(statement, &corpus.entity_names);
                    let relation = !mentions.is_empty();
                    let prefix = if relation { "evo" } else { "plan" };
                    corpus.slots.push(FactSlot {
                        key: format!(
                            "{prefix}:{identity}:ch{beat:02}:sc{:02}:st{:02}",
                            scene_index + 1,
                            state_index + 1
                        ),
                        layer: if relation {
                            Layer::Relation
                        } else {
                            Layer::Fact
                        },
                        text: format!("{}｜{statement}", scene.title),
                        valid_from: Some(story_number(tick)),
                        task_id: task_id.clone(),
                        source_path: chapter.clone(),
                        source_line: 1,
                    });
                }
            }
        }
    }
}

/// story 大纲 → 事实层事实（配置里点名的小节）。
fn collect_story_plans(
    novel_dir: &Path,
    plan_dir: &str,
    units: &[StoryUnit],
    memory: &Config,
    corpus: &mut Corpus,
) {
    let root = novel_dir.join(plan_dir);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && file_name(path).ends_with(".md"))
        .collect();
    files.sort();
    for file in files {
        let stem = file_name(&file);
        let stem = stem.trim_end_matches(".md");
        let Some((identity, default_tick)) = storytime::parse_unit_name(stem) else {
            continue;
        };
        let unit_tick = units
            .iter()
            .find(|unit| unit.identity == identity)
            .map(|unit| unit.tick);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let marker = storytime::parse_marker(&text);
        let tick = marker
            .map(|marker| marker.tick)
            .or(unit_tick)
            .unwrap_or(default_tick);
        let task_id = task_id_for(tick);
        let plan = parse_plan(&text);
        for section_name in &memory.story_fact_sections {
            let Some(section) = plan
                .sections
                .iter()
                .find(|section| &section.title == section_name)
            else {
                continue;
            };
            for (index, statement) in section.bullets.iter().enumerate() {
                corpus.slots.push(FactSlot {
                    key: format!("storyplan:{identity}:{}:{index:03}", slugify(section_name)),
                    layer: Layer::Fact,
                    text: format!("{section_name}｜{statement}"),
                    valid_from: Some(story_number(tick)),
                    task_id: task_id.clone(),
                    source_path: file.clone(),
                    source_line: 1,
                });
            }
        }
    }
}

/// 文件级标记优先，其次单元标记，最后单元名推出的默认坐标。
fn task_id_for(tick: Tick) -> Option<String> {
    Some(format!("story:{}", storytime::format_tick(tick)))
}

/// 坐标 → story 序号（`valid_from` 存的是序号，不是毫秒时间戳）。
pub fn story_number(tick: Tick) -> i64 {
    tick / storytime::TICK_SCALE
}

/// 文本里提到的已知实体名。
fn mentioned_entities(text: &str, names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| text.contains(name.as_str()))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Markdown 解析
// ---------------------------------------------------------------------------

/// 解析后的概念卡。
#[derive(Debug, Clone, Default)]
struct ParsedCard {
    title: String,
    category: String,
    card_id: String,
    aliases: Vec<String>,
    fields: BTreeMap<String, String>,
    sections: Vec<(String, String)>,
}

impl ParsedCard {
    /// 卡级事实的正文：标题 + 头部字段。
    fn header_text(&self) -> String {
        let mut lines = vec![self.title.clone()];
        for (label, value) in &self.fields {
            lines.push(format!("{label}：{value}"));
        }
        lines.join("\n")
    }
}

/// 解析概念卡：标题、头部字段、小节。
fn parse_card(text: &str) -> ParsedCard {
    let mut card = ParsedCard::default();
    let mut current_section: Option<(String, Vec<String>)> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("# ") {
            if card.title.is_empty() {
                card.title = heading.trim().to_string();
            }
            continue;
        }
        if let Some(heading) = trimmed.strip_prefix("## ") {
            if let Some((title, lines)) = current_section.take() {
                card.sections.push((title, lines.join("\n")));
            }
            current_section = Some((heading.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let Some(bullet) = bullet_text(trimmed) else {
            continue;
        };
        match current_section.as_mut() {
            None => {
                if let Some((label, value)) = split_field(&bullet) {
                    card.fields.insert(label.clone(), value.clone());
                    match label.as_str() {
                        "所属分类" => card.category = value,
                        "卡片 ID" => card.card_id = value,
                        "别名 / 英文名" => card.aliases = split_aliases(&value),
                        _ => {}
                    }
                }
            }
            Some((_, lines)) => lines.push(bullet),
        }
    }
    if let Some((title, lines)) = current_section.take() {
        card.sections.push((title, lines.join("\n")));
    }
    card
}

/// 解析大纲：故事级小节 + 场景级小节。
#[derive(Debug, Clone, Default)]
struct ParsedPlan {
    /// 顶层小节（story 大纲用）。
    sections: Vec<PlanSection>,
    /// 场景（章级施工图用）。
    scenes: Vec<PlanScene>,
}

/// 一个大纲小节。
#[derive(Debug, Clone, Default)]
struct PlanSection {
    title: String,
    bullets: Vec<String>,
}

/// 一个场景。
#[derive(Debug, Clone, Default)]
struct PlanScene {
    title: String,
    /// 状态变化（`状态变化` 字段的条目）。
    states: Vec<String>,
}

/// 解析大纲文本。
fn parse_plan(text: &str) -> ParsedPlan {
    let mut plan = ParsedPlan::default();
    let mut current: Option<(String, Vec<PlanItem>)> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            if let Some((title, items)) = current.take() {
                push_section(&mut plan, title, items);
            }
            current = Some((heading.trim().to_string(), Vec::new()));
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((indent, bullet)) = bullet_with_indent(line) else {
            continue;
        };
        let Some((_, items)) = current.as_mut() else {
            continue;
        };
        let (label, value) = split_field(&bullet).unwrap_or_default();
        if indent == 0 {
            let mut values = Vec::new();
            if !value.is_empty() {
                values.push(value);
            }
            items.push(PlanItem {
                label,
                text: bullet,
                values,
            });
        } else if let Some(item) = items.last_mut() {
            item.values.push(bullet);
        }
    }
    if let Some((title, items)) = current.take() {
        push_section(&mut plan, title, items);
    }
    plan
}

/// 大纲小节里的一条目。
#[derive(Debug, Clone, Default)]
struct PlanItem {
    /// `标签：` 里的标签（没有则为空）。
    label: String,
    /// 原始文本。
    text: String,
    /// 取值（行内取值 + 缩进子条目）。
    values: Vec<String>,
}

/// 把一个小节的条目分派到「场景」或「顶层小节」。
fn push_section(plan: &mut ParsedPlan, title: String, items: Vec<PlanItem>) {
    if title.starts_with("Scene") || title.starts_with("scene") {
        let mut scene = PlanScene {
            title: title.clone(),
            states: Vec::new(),
        };
        for item in items {
            if item.label.contains("状态变化") {
                scene.states.extend(item.values);
            }
        }
        plan.scenes.push(scene);
        return;
    }
    let mut section = PlanSection {
        title,
        bullets: Vec::new(),
    };
    for item in items {
        if item.values.is_empty() {
            section.bullets.push(item.text);
            continue;
        }
        if item.label.is_empty() {
            section.bullets.extend(item.values);
            continue;
        }
        for value in item.values {
            section.bullets.push(format!("{}：{value}", item.label));
        }
    }
    plan.sections.push(section);
}

/// 取一个字段小节的条目（关系小节用）。
fn section_bullets(section: &str) -> Vec<String> {
    section
        .lines()
        .filter_map(|line| bullet_text(line.trim()))
        .collect()
}

/// 取 `- ` / `N. ` 开头的正文。
fn bullet_text(trimmed: &str) -> Option<String> {
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest.trim().to_string());
    }
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let rest = &trimmed[digits.len()..];
        if let Some(rest) = rest.strip_prefix(". ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// 取条目文本与缩进层级。
fn bullet_with_indent(line: &str) -> Option<(usize, String)> {
    let indent = line.len() - line.trim_start().len();
    let trimmed = line.trim();
    bullet_text(trimmed).map(|bullet| (indent, bullet))
}

/// 拆 `标签：取值`。
fn split_field(bullet: &str) -> Option<(String, String)> {
    let (label, value) = bullet.split_once('：')?;
    Some((label.trim().to_string(), value.trim().to_string()))
}

/// 拆关系条目 `对象：描述`。
fn split_relation(bullet: &str) -> Option<(String, String)> {
    let (target, description) = bullet.split_once('：')?;
    let target = target.trim();
    let description = description.trim();
    if target.is_empty() || description.is_empty() {
        return None;
    }
    Some((target.to_string(), description.to_string()))
}

/// 别名拆分（`A / B` → `["A", "B"]`）。
fn split_aliases(value: &str) -> Vec<String> {
    value
        .split(['/', '、', '，'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// 取反引号里的路径。
fn extract_backticked(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else {
            break;
        };
        items.push(after[..end].trim().to_string());
        rest = &after[end + 1..];
    }
    items
}

/// 正文段落（跳过标题行、空行与 HTML 注释）。
fn paragraphs(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !current.is_empty() {
                blocks.push(current.join(" "));
                current.clear();
            }
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with("<!--") {
            continue;
        }
        current.push(trimmed);
    }
    if !current.is_empty() {
        blocks.push(current.join(" "));
    }
    blocks
        .into_iter()
        .map(|block| block.trim().to_string())
        .filter(|block| !block.is_empty())
        .collect()
}

/// 槽位键里的名字：保留 ASCII 字母数字，其余换成 `-`。
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// 文件名。
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 递归收集目录下的 markdown 文件（排序后返回）。
fn markdown_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            files.extend(markdown_files_recursive(&path));
        } else if file_name(&path).ends_with(".md") {
            files.push(path);
        }
    }
    files.sort();
    files
}
