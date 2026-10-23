//! 一致性（consistency）模块。
//!
//! 为小说文件构建并查询轻量一致性搜索索引：SQLite（rusqlite，bundled
//! FTS5）+ 反馈 JSONL 回路。子命令：
//! build / search / entity / facts / story-facts / tension / conflicts /
//! feedback-add / feedback-summary / review-queue / catalog / suspects /
//! alignment（13 支）。
//!
//! 输出文案固定（含中文措辞，逐字节稳定）；SQL 参数化；
//! 每命令退出前提交，语句级 autocommit 下
//! 最终状态一致。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use chrono::{SecondsFormat, Utc};
use regex::Regex;
use rusqlite::{params, Connection, ToSql};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::stats::Ctr;

// ---------------------------------------------------------------------------
// 常量（字面量固定）
// ---------------------------------------------------------------------------

pub const CONSISTENCY_MODULE_TARGET: &str = "novel-review consistency";
pub const RULES_TEMPLATE_TARGET: &str = "configs/rules/review.yaml#draft.template_rules";
pub const BOOK_DRAFT_RULES_TARGET: &str = "novel1/rules/draft.md";
pub const CONSISTENCY_CLI: &[&str] = &["novel-review", "consistency"];

pub const FEEDBACK_DECISIONS: &[&str] = &["confirmed", "false_positive", "designed_keep", "watch"];
pub const FEEDBACK_FACETS: &[&str] = &[
    "rhythm",
    "voice",
    "motif",
    "scene_callback",
    "register",
    "naming",
    "irony",
    "state_progression",
    "extractor_noise",
    "scope_drift",
];

pub const INJURY_NEGATIVE_TERMS: &[&str] = &[
    "受伤",
    "流血",
    "出血",
    "伤口",
    "裂开",
    "擦伤",
    "扭伤",
    "包扎",
    "咳血",
    "发白",
    "发烧",
    "疼得",
    "止血贴",
];

pub const INJURY_STABLE_TERMS: &[&str] = &[
    "没事",
    "稳住",
    "恢复",
    "缓过",
    "站稳",
    "止住",
    "轻伤",
    "能走",
    "还能打",
];

pub const EQUIPMENT_DAMAGED_TERMS: &[&str] = &[
    "裂开",
    "擦痕",
    "损坏",
    "失灵",
    "熄灭",
    "坏了",
    "断掉",
    "烧毁",
    "暴露体积",
];

pub const EQUIPMENT_ACTIVE_TERMS: &[&str] = &[
    "展开",
    "变形",
    "启动",
    "亮起",
    "抬起",
    "展开快",
    "护住",
    "挡在",
    "接口",
];

pub const GOAL_ASSIGNED_TERMS: &[&str] = &["任务", "委托", "命令", "要求", "安排", "交给", "负责"];

pub const GOAL_CHANGED_TERMS: &[&str] = &[
    "改成",
    "转而",
    "临时改",
    "改口",
    "换成",
    "不再是",
    "目标变成",
];

pub const GOAL_COMPLETED_TERMS: &[&str] = &["完成", "办完", "解决", "结束", "交差", "收尾", "达成"];

pub const RELATIONSHIP_CLOSE_TERMS: &[&str] = &[
    "信任",
    "护住",
    "并肩",
    "默认",
    "配合",
    "接住",
    "愿意跟",
    "攥住",
    "按回去",
    "摁回去",
    "扯下来",
    "拎起来",
    "认你",
    "替他扛",
    "替她扛",
    "挡在前面",
    "挡在身前",
    "拉住",
    "拉回来",
    "接应",
    "护在前面",
];

pub const RELATIONSHIP_DISTANT_TERMS: &[&str] = &[
    "提防",
    "怀疑",
    "警惕",
    "疏远",
    "冷淡",
    "不信",
    "避开",
    "甩开",
    "推开",
    "别碰",
    "闭嘴",
    "少来",
    "离远点",
];

/// `FACT_TERM_GROUPS`（插入序 = 字面量顺序）。
const FACT_TERM_GROUPS: &[(&str, &[&str])] = &[
    ("injury_negative", INJURY_NEGATIVE_TERMS),
    ("injury_stable", INJURY_STABLE_TERMS),
    ("equipment_damaged", EQUIPMENT_DAMAGED_TERMS),
    ("equipment_active", EQUIPMENT_ACTIVE_TERMS),
    ("goal_assigned", GOAL_ASSIGNED_TERMS),
    ("goal_changed", GOAL_CHANGED_TERMS),
    ("goal_completed", GOAL_COMPLETED_TERMS),
    ("relationship_close", RELATIONSHIP_CLOSE_TERMS),
    ("relationship_distant", RELATIONSHIP_DISTANT_TERMS),
];

// ---------------------------------------------------------------------------
// 正则（regex 编译；`\d` 仅匹配 ASCII 数字，本仓文件名均为 ASCII 数字）
// ---------------------------------------------------------------------------

static WS_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("空白正则"));
static ALIASES_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[，,、；;]+").expect("别名分隔正则"));
static HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#\s+(.*)$").expect("卡片标题正则"));
static FIELD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^- ([^：]+)：\s*(.*)$").expect("卡片字段正则"));
static STORY_PLAN_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(story)-(\d+[a-z]?)").expect("story plan 文件名正则"));
static INTERLUDE_PLAN_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(interlude)-(\d+)").expect("interlude plan 文件名正则"));
static STORY_CHAPTER_FILE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(story\d+|interlude\d+)-ch\d+").expect("story/chapter 文件名正则")
});
static CHAPTER_ONLY_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^ch\d+\.md$").expect("ch 文件名正则"));
static FACT_SEGMENT_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[。！？!?；;\n]+").expect("fact 分段正则"));
static NEGATED_RELATION_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(不等于|不是|并非|非)\s*信任",
        r"信任\s*(不了|不起来|不起|不能)",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("否定关系正则"))
    .collect()
});
static ARC_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)arc\d+").expect("arc 目录正则"));
static STORY_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)story\d+").expect("story 目录正则"));
static INTERLUDE_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)interlude\d+").expect("interlude 目录正则"));
static CHAPTER_PART_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)ch\d+\.md").expect("ch 目录正则"));
static ALIGN_PLAN_ONLY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"plan_only=([^;]+)").expect("plan_only 摘要正则"));
static ALIGN_DRAFT_ONLY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"draft_only=([^;]+)").expect("draft_only 摘要正则"));

// ---------------------------------------------------------------------------
// 基础类型
// ---------------------------------------------------------------------------

/// 实体。
#[derive(Debug, Clone)]
pub struct Entity {
    pub category: String,
    pub card_path: PathBuf,
    pub title: String,
    pub card_id: String,
    pub names: Vec<String>,
}

/// 证据类型（`evidence_kind` 字段取值）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EvidenceKind {
    /// 事实跳变证据。
    #[default]
    Fact,
    /// 称呼口径漂移证据。
    Alias,
    /// plan/draft 实体对齐缺口证据。
    Alignment,
}

/// 冲突候选行（键值对行模型）。
#[derive(Debug, Clone, Default)]
pub struct ConflictRow {
    pub category: String,
    pub story: String,
    pub title: String,
    pub entity_category: String,
    pub summary: String,
    /// `fact` / `alias` / `alignment`。
    pub evidence_kind: EvidenceKind,
    /// fact 行携带；alias/alignment 行为空。
    pub fact_types: Vec<String>,
    /// high / medium / low。
    pub confidence: String,
    pub support_note: String,
    pub feedback_decision: String,
    pub feedback_facet: String,
    pub feedback_note: String,
    pub feedback_updated_at: String,
}

/// 关系动作词共现对（`query_story_relationship_pair_rows` 输出）。
#[derive(Debug, Clone)]
pub struct PairRow {
    pub story: String,
    pub path: String,
    pub chapter: Option<String>,
    pub line_start: i64,
    pub line_end: i64,
    pub text: String,
    pub left_title: String,
    pub right_title: String,
    pub close_cues: String,
    pub distant_cues: String,
}

/// story 级 plan/draft 实体覆盖行。
#[derive(Debug, Clone)]
pub struct AlignmentRow {
    pub story: String,
    pub plan_entities: i64,
    pub draft_entities: i64,
    pub plan_only_entities: Option<String>,
    pub draft_only_entities: Option<String>,
}

/// 称呼漂移行。
#[derive(Debug, Clone)]
pub struct AliasDriftRow {
    pub story: String,
    pub title: String,
    pub category: String,
    pub draft_aliases: Option<String>,
    pub plan_aliases: Option<String>,
}

/// 状态张力行（受伤/装备）。
#[derive(Debug, Clone)]
pub struct TensionRow {
    pub story: String,
    pub title: String,
    pub category: String,
    pub injury_negative: Option<String>,
    pub injury_stable: Option<String>,
    pub equipment_damaged: Option<String>,
    pub equipment_active: Option<String>,
}

/// 目标张力行。
#[derive(Debug, Clone)]
pub struct GoalTensionRow {
    pub story: String,
    pub title: String,
    pub category: String,
    pub goal_assigned: Option<String>,
    pub goal_changed: Option<String>,
    pub goal_completed: Option<String>,
}

/// 关系张力行。
#[derive(Debug, Clone)]
pub struct RelationshipTensionRow {
    pub story: String,
    pub title: String,
    pub category: String,
    pub relationship_close: Option<String>,
    pub relationship_distant: Option<String>,
}

/// 通用证据行。
#[derive(Debug, Clone)]
pub struct EvidenceRow {
    pub path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub text: String,
}

/// fact 证据行。
#[derive(Debug, Clone)]
pub struct FactEvidenceRow {
    pub path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub text: String,
    pub fact_type: String,
    pub cue: String,
}

/// 称呼证据行。
#[derive(Debug, Clone)]
pub struct AliasEvidenceRow {
    pub path: String,
    pub line_start: i64,
    pub line_end: i64,
    pub text: String,
    pub matched_name: String,
}

/// 人工复核行动（`build_pending_review_actions` 条目）。
#[derive(Debug, Clone)]
pub struct ReviewAction {
    pub story: String,
    pub category: String,
    pub title: String,
    pub confidence: String,
    pub focus: String,
    pub command: String,
}

/// 反馈沉淀建议（`build_feedback_backlog` 条目）。
#[derive(Debug, Clone)]
pub struct BacklogItem {
    pub target: String,
    pub reason: String,
}

/// 反馈 JSONL 记录（键保序 map，值一律字符串化）。
pub type FeedbackRecord = JsonMap<String, JsonValue>;

/// `summarize_feedback` 输出。
#[derive(Debug, Default)]
pub struct FeedbackSummary {
    pub entries: BTreeMap<String, FeedbackRecord>,
    pub history: Vec<FeedbackRecord>,
    pub conflict_rows: Vec<ConflictRow>,
    pub decision_counter: Ctr,
    pub category_counter: Ctr,
    pub story_counter: Ctr,
    pub facet_counter: Ctr,
    pub unresolved: Vec<ConflictRow>,
    pub pending_actions: Vec<ReviewAction>,
    pub backlog: Vec<BacklogItem>,
    pub unresolved_by_story: Ctr,
    pub story_filter: Option<String>,
}

/// `build_story_conflict_snapshot_from_path` 输出（快照全字段）。
#[derive(Debug)]
pub struct StoryConflictSnapshot {
    pub available: bool,
    /// novel_dir_not_found / db_not_found / story_not_found。
    pub reason: Option<String>,
    pub novel_dir: Option<PathBuf>,
    pub db_path: Option<PathBuf>,
    pub feedback_path: Option<PathBuf>,
    pub story: Option<String>,
    pub review_queue_command: Option<String>,
    pub feedback_summary_command: Option<String>,
    pub rows: Vec<ConflictRow>,
    pub decision_counter: Ctr,
    pub category_counter: Ctr,
    pub facet_counter: Ctr,
    pub pending_rows: Vec<ConflictRow>,
    /// `pending_rows.len()`（`len(pending_rows)`）。
    pub pending_count: usize,
    pub pending_actions: Vec<ReviewAction>,
    pub global_feedback_backlog: Vec<BacklogItem>,
}

// ---------------------------------------------------------------------------
// 基础工具
// ---------------------------------------------------------------------------

/// `normalize_whitespace`：连续空白压成单空格并去首尾空白。
pub fn normalize_whitespace(text: &str) -> String {
    WS_RE.replace_all(text, " ").trim().to_string()
}

/// `prefix_chars`：前 n 个码点。
fn prefix_chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `split_pipe_values`。
pub fn split_pipe_values(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if value.is_empty() {
        return Vec::new();
    }
    value
        .split('|')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// `split_pipe_values_str`：非 Option 版本（对 `""` 返回空列表）。
pub fn split_pipe_values_str(value: &str) -> Vec<String> {
    split_pipe_values(Some(value))
}

/// `default_feedback_path_from_db`。
pub fn default_feedback_path_from_db(db_path: &Path) -> PathBuf {
    parent_or_dot(db_path).join("review-feedback.jsonl")
}

pub(crate) fn parent_or_dot(path: &Path) -> PathBuf {
    path.parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `find_novel_dir`：沿父目录链找 `novel*` 且含 `drafts/` 的目录。
pub fn find_novel_dir(path: &Path) -> Option<PathBuf> {
    let current = if path.exists() {
        std::fs::canonicalize(path).ok()?
    } else {
        path.to_path_buf()
    };
    let mut cursor = current;
    loop {
        if is_novel_root(&cursor) {
            return Some(cursor);
        }
        let parent = cursor.parent()?;
        if parent == cursor.as_path() {
            return None;
        }
        cursor = parent.to_path_buf();
    }
}

fn is_novel_root(candidate: &Path) -> bool {
    let name = candidate.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("novel") && candidate.join("drafts").is_dir()
}

/// `conflict_key`。
pub fn conflict_key(row: &ConflictRow) -> String {
    [
        row.category.as_str(),
        row.story.as_str(),
        row.title.as_str(),
        row.entity_category.as_str(),
        row.summary.as_str(),
    ]
    .join("||")
}

/// `summary_fragment`。
pub fn summary_fragment(summary: &str, max_chars: usize) -> String {
    let mut s = summary.to_string();
    for token in [" ; ", "；", " -> ", ",", "，"] {
        if let Some(pos) = s.find(token) {
            s.truncate(pos);
            break;
        }
    }
    prefix_chars(&normalize_whitespace(&s), max_chars)
}

/// `build_pending_review_focus`。
pub fn build_pending_review_focus(row: &ConflictRow) -> String {
    let evidence_kind = match row.evidence_kind {
        EvidenceKind::Fact => "fact",
        EvidenceKind::Alias => "alias",
        EvidenceKind::Alignment => "alignment",
    };
    if evidence_kind == "fact" {
        return "先回看 fact cue 和上下文段，判断这是真跳变还是阶段推进。".to_string();
    }
    if evidence_kind == "alias" {
        return "先对照 plan / draft 两侧称呼，判断是口径漂移还是有意压拍。".to_string();
    }
    if evidence_kind == "alignment" {
        return "先看 plan_only / draft_only 实体，判断是正文漏落还是施工图写偏。".to_string();
    }
    if row.category.contains("relationship") {
        return "先回看关系动作词和同段人物共现，判断是关系转冷还是抽取误绑。".to_string();
    }
    if row.category.contains("goal") {
        return "先回看任务口径和章末动作，判断是目标改口还是正常推进。".to_string();
    }
    "先回看相关证据段，再决定是 confirmed、false_positive 还是 designed_keep。".to_string()
}

/// `shlex.quote`（POSIX 引用规则）。
fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = value.bytes().all(|b| {
        matches!(
            b,
            b'a'..=b'z'
                | b'A'..=b'Z'
                | b'0'..=b'9'
                | b'_'
                | b'@'
                | b'%'
                | b'+'
                | b'='
                | b':'
                | b','
                | b'.'
                | b'/'
                | b'-'
        )
    });
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// `build_feedback_command`（`decision` 缺省 `watch`）。
pub fn build_feedback_command(db_path: &Path, row: &ConflictRow, decision: Option<&str>) -> String {
    let decision = decision.unwrap_or("watch");
    let command_root = find_novel_dir(db_path).unwrap_or_else(|| db_path.to_path_buf());
    let name = command_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let root_label = if name.starts_with("novel") {
        name.to_string()
    } else {
        command_root.display().to_string()
    };
    let mut parts: Vec<String> = CONSISTENCY_CLI.iter().map(|s| s.to_string()).collect();
    parts.extend([
        "feedback-add".to_string(),
        root_label,
        "--category".to_string(),
        row.category.clone(),
        "--story".to_string(),
        row.story.clone(),
        "--title".to_string(),
        row.title.clone(),
        "--decision".to_string(),
        decision.to_string(),
    ]);
    let fragment = summary_fragment(&row.summary, 18);
    if !fragment.is_empty() {
        parts.push("--summary-contains".to_string());
        parts.push(fragment);
    }
    parts
        .iter()
        .map(|p| shlex_quote(p))
        .collect::<Vec<String>>()
        .join(" ")
}

/// `build_pending_review_actions`（`limit` 缺省 6）。
pub fn build_pending_review_actions(
    db_path: &Path,
    rows: &[ConflictRow],
    limit: usize,
) -> Vec<ReviewAction> {
    rows.iter()
        .take(limit)
        .map(|row| ReviewAction {
            story: row.story.clone(),
            category: row.category.clone(),
            title: row.title.clone(),
            confidence: row.confidence.clone(),
            focus: build_pending_review_focus(row),
            command: build_feedback_command(db_path, row, None),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CLI（clap 子命令面，参数名/默认值固定）
// ---------------------------------------------------------------------------

use clap::Subcommand;

/// `consistency` 子命令（13 个子命令）。
#[derive(Subcommand)]
pub enum ConsistencyCmd {
    /// 为小说目录构建索引
    Build {
        /// 小说目录（例如 novel1）
        novel_dir: PathBuf,
        /// 输出 SQLite 路径；缺省为 <novel_dir>/research/consistency/consistency.sqlite3
        #[arg(long)]
        db_path: Option<PathBuf>,
        /// 规则文件路径（读取 `paths` 节）；缺省为 configs/rules/review.yaml
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 全文检索已索引段落
    Search {
        /// SQLite 数据库路径、consistency 目录或 novel 目录
        db_path: PathBuf,
        /// FTS 检索词或短语
        term: String,
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// 查看已知实体的出现段落
    Entity {
        db_path: PathBuf,
        /// 实体标题或别名
        name: String,
        #[arg(long, default_value_t = 30)]
        limit: i64,
    },
    /// 查看已知实体的事实候选
    Facts {
        db_path: PathBuf,
        name: String,
        #[arg(long, default_value_t = 30)]
        limit: i64,
    },
    /// 查看单条 story 的聚合事实候选
    StoryFacts {
        db_path: PathBuf,
        /// story id（例如 story1 或 interlude1）
        story: String,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// 查看 story 级状态张力候选
    Tension {
        db_path: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
    /// 查看冲突候选与证据
    Conflicts {
        db_path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// 反馈 JSONL 路径；缺省为 <db 目录>/review-feedback.jsonl
        #[arg(long)]
        feedback_path: Option<PathBuf>,
    },
    /// 记录某条冲突的人工复核决定
    FeedbackAdd {
        db_path: PathBuf,
        #[arg(long, required = true)]
        category: String,
        #[arg(long, required = true)]
        story: String,
        /// 实体标题；story 级漂移用 -
        #[arg(long, required = true)]
        title: String,
        /// confirmed / false_positive / designed_keep / watch
        #[arg(long, required = true)]
        decision: String,
        /// 可选切面（rhythm / voice / motif / scene_callback / register / naming / irony /
        /// state_progression / extractor_noise / scope_drift）
        #[arg(long, default_value = "")]
        facet: String,
        /// 简短人工备注
        #[arg(long, default_value = "")]
        note: String,
        /// 用于区分同组行的 summary 片段
        #[arg(long)]
        summary_contains: Option<String>,
        /// 反馈 JSONL 路径；缺省为 <db 目录>/review-feedback.jsonl
        #[arg(long)]
        feedback_path: Option<PathBuf>,
    },
    /// 汇总已记录的反馈
    FeedbackSummary {
        db_path: PathBuf,
        #[arg(long, default_value_t = 200)]
        limit: i64,
        /// 可选 story id（例如 story3）
        #[arg(long)]
        story: Option<String>,
        #[arg(long)]
        feedback_path: Option<PathBuf>,
    },
    /// 打印待复核队列（含证据）
    ReviewQueue {
        db_path: PathBuf,
        #[arg(long)]
        story: Option<String>,
        #[arg(long, default_value_t = 8)]
        limit: i64,
        #[arg(long)]
        feedback_path: Option<PathBuf>,
    },
    /// 列出已索引实体与提及数
    Catalog {
        db_path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// 打印疑似称呼混用 / 口径漂移
    Suspects {
        db_path: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// 打印 story 级 plan/draft 实体覆盖
    Alignment {
        db_path: PathBuf,
        #[arg(long, default_value_t = 100)]
        limit: i64,
    },
}

fn resolve_feedback_path(db_path: &Path, explicit: Option<&Path>) -> PathBuf {
    explicit
        .map(PathBuf::from)
        .unwrap_or_else(|| default_feedback_path_from_db(&resolve_db_path(db_path)))
}

/// 子命令分发（返回进程退出码）。
pub fn run(cmd: &ConsistencyCmd) -> Result<i32> {
    use ConsistencyCmd::*;
    match cmd {
        Build {
            novel_dir,
            db_path,
            rules,
        } => {
            let db_path = db_path.clone().unwrap_or_else(|| {
                novel_dir
                    .join("research")
                    .join("consistency")
                    .join("consistency.sqlite3")
            });
            let rules_path = rules
                .clone()
                .unwrap_or_else(crate::config::default_rules_path);
            let paths = crate::config::load_paths(&rules_path);
            build_index(novel_dir, &db_path, &paths)?;
            println!("{}", db_path.display());
            Ok(0)
        }
        Search {
            db_path,
            term,
            limit,
        } => run_db_query(db_path, |conn| print_search_results(conn, term, *limit)),
        Entity {
            db_path,
            name,
            limit,
        } => run_db_query(db_path, |conn| print_entity_results(conn, name, *limit)),
        Facts {
            db_path,
            name,
            limit,
        } => run_db_query(db_path, |conn| print_entity_facts(conn, name, *limit)),
        StoryFacts {
            db_path,
            story,
            limit,
        } => run_db_query(db_path, |conn| print_story_facts(conn, story, *limit)),
        Catalog { db_path, limit } => {
            run_db_query(db_path, |conn| print_entity_catalog(conn, *limit))
        }
        Suspects { db_path, limit } => run_db_query(db_path, |conn| print_suspects(conn, *limit)),
        Alignment { db_path, limit } => {
            run_db_query(db_path, |conn| print_story_alignment(conn, *limit))
        }
        Tension { db_path, limit } => {
            run_db_query(db_path, |conn| print_story_tension(conn, *limit))
        }
        Conflicts {
            db_path,
            limit,
            feedback_path,
        } => run_db_query(db_path, |conn| {
            let feedback = resolve_feedback_path(db_path, feedback_path.as_deref());
            print_conflicts(conn, *limit, Some(&feedback))
        }),
        FeedbackAdd {
            db_path,
            category,
            story,
            title,
            decision,
            facet,
            note,
            summary_contains,
            feedback_path,
        } => {
            let feedback = resolve_feedback_path(db_path, feedback_path.as_deref());
            let req = FeedbackRequest {
                category: category.clone(),
                story: story.clone(),
                title: title.clone(),
                decision: decision.clone(),
                facet: facet.clone(),
                note: note.clone(),
                summary_contains: summary_contains.clone(),
            };
            let mut rc = 0i32;
            run_db_query(db_path, |conn| {
                match write_feedback(conn, &feedback, &req)? {
                    None => {
                        println!("{}", feedback.display());
                        println!("{category} :: {story} :: {title} :: decision={decision}");
                    }
                    Some(message) => {
                        eprintln!("{message}");
                        rc = 1;
                    }
                }
                Ok(())
            })?;
            Ok(rc)
        }
        FeedbackSummary {
            db_path,
            limit,
            story,
            feedback_path,
        } => run_db_query(db_path, |conn| {
            let feedback = resolve_feedback_path(db_path, feedback_path.as_deref());
            print_feedback_summary(conn, &feedback, *limit, story.as_deref())
        }),
        ReviewQueue {
            db_path,
            story,
            limit,
            feedback_path,
        } => run_db_query(db_path, |conn| {
            let feedback = resolve_feedback_path(db_path, feedback_path.as_deref());
            print_review_queue(conn, &feedback, story.as_deref(), *limit)
        }),
    }
}

fn run_db_query(db_path: &Path, query: impl FnOnce(&Connection) -> Result<()>) -> Result<i32> {
    let resolved = resolve_db_path(db_path);
    let conn = open_db(&resolved)?;
    query(&conn)?;
    Ok(0)
}

mod conflicts;
mod feedback;
mod print;
mod query;
mod scan;

pub use conflicts::{
    apply_feedback, build_story_conflict_snapshot, build_story_conflict_snapshot_from_path,
    collect_conflict_rows, query_conflict_rows, summarize_feedback, write_feedback,
    FeedbackRequest,
};
pub(crate) use feedback::record_str;
pub use feedback::{
    append_feedback_entry, build_feedback_backlog, filter_feedback_history_by_story,
    load_feedback_entries, read_feedback_history,
};
pub use print::{
    print_conflict_evidence, print_conflicts, print_entity_catalog, print_entity_facts,
    print_entity_results, print_feedback_summary, print_review_queue, print_search_results,
    print_story_alignment, print_story_facts, print_story_tension, print_suspects,
};
pub use query::{
    collect_relationship_cues, has_relationship_pronoun_bridge, query_fact_support_summary,
    query_story_alias_drift_rows, query_story_alias_evidence, query_story_alignment_evidence,
    query_story_alignment_gap_rows, query_story_alignment_rows, query_story_goal_tension_rows,
    query_story_relationship_pair_rows, query_story_relationship_tension_rows,
    query_story_tension_evidence, query_story_tension_rows, score_alias_confidence,
    score_alignment_confidence, score_fact_confidence,
};
pub use scan::{
    build_index, classify_document, collect_local_fact_cues, extract_names, infer_title_variants,
    is_negated_relationship_segment, iter_documents, load_entities, open_db, parse_field_map,
    resolve_db_path, split_fact_segments, split_passages,
};
