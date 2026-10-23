//! 章节施工图（chapter-plan）与草稿的功能/章末对齐信号。
//!
//! 由 `reports::scorecard` 消费（`reports-scorecard` 子命令）；报告只读，
//! 全部中文文案固定。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audit::draft::{Analysis, Counter};
use crate::audit::plan::{self as plan_audit, PlanEngine};

/// 草稿章功能词表（照抄 `DRAFT_FUNCTION_RULES` 字面量与顺序）。
pub const DRAFT_FUNCTION_RULES: &[(&str, &[&str])] = &[
    (
        "conflict",
        &[
            "枪", "火力", "埋伏", "子弹", "追", "拦", "伤", "血", "打", "炸",
        ],
    ),
    (
        "investigation",
        &[
            "线索", "证据", "坐标", "记录", "名单", "异常", "确认", "查", "归档",
        ],
    ),
    (
        "relationship",
        &[
            "对视", "沉默", "笑", "嘴硬", "护住", "搭档", "信任", "回嘴", "安慰",
        ],
    ),
    (
        "procedure",
        &[
            "安检", "排队", "窗口", "手续", "通道", "账单", "登记", "权限",
        ],
    ),
    (
        "exposition",
        &[
            "解释", "说明", "分析", "知道", "明白", "讨论", "复盘", "判断",
        ],
    ),
    (
        "movement",
        &[
            "出城", "回城", "进城", "上车", "下车", "抵达", "离开", "赶到", "入口",
        ],
    ),
];

/// 草稿章末功能词表（照抄 `DRAFT_ENDING_RULES` 字面量与顺序）。
pub const DRAFT_ENDING_RULES: &[(&str, &[&str])] = &[
    (
        "action_aftershock",
        &["撤", "压上", "挡在", "补位", "继续追", "转移", "收拢"],
    ),
    (
        "new_info",
        &[
            "线索", "名单", "坐标", "名字", "短码", "短讯", "消息", "记录",
        ],
    ),
    (
        "external_threat",
        &[
            "异响", "危险", "热源", "追兵", "枪火", "火力", "报警", "封锁",
        ],
    ),
    (
        "relationship_turn",
        &[
            "沉默",
            "看了她一眼",
            "护住",
            "松口",
            "翻脸",
            "笑了笑",
            "回嘴",
        ],
    ),
    (
        "procedure_pressure",
        &[
            "安检", "窗口", "账单", "手续", "权限", "通报", "资格", "登记",
        ],
    ),
    (
        "foreshadow_flash",
        &[
            "不该",
            "熟悉",
            "异常",
            "多了一处",
            "旧标记",
            "像是",
            "闪了一下",
        ],
    ),
    (
        "self_realization",
        &["意识到", "明白", "知道", "想起", "终于懂"],
    ),
];

/// `count_rule_hits`：按词表顺序赋值各 label 的子串命中和（重叠计数约定）。
#[must_use]
pub fn count_rule_hits(text: &str, rules: &[(&str, &[&str])]) -> Counter {
    let mut counter = Counter::default();
    for (label, terms) in rules {
        let total = terms
            .iter()
            .map(|term| text.matches(*term).count())
            .sum::<usize>();
        counter.add_n(label, total);
    }
    counter
}

/// `pick_top_label`：count>0 中按 (-count, name) 取首；全 0 时返回默认值。
#[must_use]
pub fn pick_top_label(counter: &Counter, default: &str) -> String {
    let positive: Vec<(String, usize)> = counter
        .entries()
        .iter()
        .filter(|(_, count)| *count > 0)
        .cloned()
        .collect();
    if positive.is_empty() {
        return default.to_string();
    }
    let mut ranked = positive;
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .next()
        .map(|(name, _)| name)
        .unwrap_or_else(|| default.to_string())
}

/// `chapter_plan_path_for_draft`：`story1` 直挂，其余 story 以 `{story}-{chapter}` 挂。
#[must_use]
pub fn chapter_plan_path_for_draft(
    draft_path: &Path,
    novel_dir: &Path,
    paths: &crate::config::PathsConfig,
) -> Option<PathBuf> {
    let relative = draft_path
        .strip_prefix(novel_dir.join(&paths.drafts))
        .ok()?;
    let parts: Vec<_> = relative.components().collect();
    if parts.len() < 3 {
        return None;
    }
    let os_str_to_string =
        |component: &std::path::Component| component.as_os_str().to_string_lossy().into_owned();
    let arc = os_str_to_string(&parts[0]);
    let story = os_str_to_string(&parts[1]);
    let chapter = draft_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)?;
    if story == "story1" {
        Some(novel_dir.join(&paths.chapter_plan).join(arc).join(chapter))
    } else {
        Some(
            novel_dir
                .join(&paths.chapter_plan)
                .join(arc)
                .join(format!("{story}-{chapter}")),
        )
    }
}

/// 施工图侧信号（对应 `collect_plan_signals` 返回值）。
#[derive(Debug, Clone, Default)]
pub struct PlanSignals {
    pub available: bool,
    pub chapter_function: String,
    pub ending_function: String,
    /// `path` 键（仅 available 时有；`str(chapter_plan_path)`）。
    pub path: Option<String>,
}

fn join_bullet_texts(section: &[plan_audit::BodyLine]) -> String {
    plan_audit::bullet_lines(section)
        .iter()
        .map(|(_, text)| text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `collect_plan_signals`：章节功能 / 章末功能标签推断（施工图缺失时 `missing`）。
pub fn collect_plan_signals(engine: &PlanEngine, chapter_plan_path: &Path) -> Result<PlanSignals> {
    if !chapter_plan_path.exists() {
        return Ok(PlanSignals {
            available: false,
            chapter_function: "missing".to_string(),
            ending_function: "missing".to_string(),
            path: None,
        });
    }
    let text = std::fs::read_to_string(chapter_plan_path)
        .with_context(|| format!("无法读取 {}", chapter_plan_path.display()))?;
    let lines: Vec<&str> = text.lines().collect();
    let headings = plan_audit::parse_headings(&lines);
    let sections = plan_audit::collect_section_lines(&lines, &headings);
    let (_function_name, function_section) = plan_audit::find_section(&sections, &["本章功能"]);
    let function_text = {
        let bullets = join_bullet_texts(function_section);
        if bullets.is_empty() {
            plan_audit::section_text(function_section)
        } else {
            bullets
        }
    };
    let ending_group: Vec<&str> = engine
        .chapter_ending_group()
        .iter()
        .map(String::as_str)
        .collect();
    let (_ending_name, ending_section) = plan_audit::find_section(&sections, &ending_group);
    let ending_text = {
        let bullets = join_bullet_texts(ending_section);
        if bullets.is_empty() {
            plan_audit::section_text(ending_section)
        } else {
            bullets
        }
    };
    Ok(PlanSignals {
        available: true,
        chapter_function: plan_audit::detect_function_label(
            &function_text,
            engine.chapter_function_rules(),
        ),
        ending_function: plan_audit::detect_function_label(
            &ending_text,
            engine.ending_function_rules(),
        ),
        path: Some(chapter_plan_path.display().to_string()),
    })
}

/// 草稿侧信号（对应 `infer_draft_signals` 返回值）。
#[derive(Debug, Clone, Default)]
pub struct DraftSignals {
    pub chapter_function: String,
    pub ending_function: String,
    pub chapter_counter: Counter,
    pub ending_counter: Counter,
}

/// `infer_draft_signals`：整章功能词表命中 + 场面/战斗/对白/跟踪词加成。
pub fn infer_draft_signals(draft_path: &Path, analysis: &Analysis) -> Result<DraftSignals> {
    let text = std::fs::read_to_string(draft_path)
        .with_context(|| format!("无法读取 {}", draft_path.display()))?;
    let mut function_counter = count_rule_hits(&text, DRAFT_FUNCTION_RULES);
    let mut ending_counter = count_rule_hits(&analysis.ending.tail_excerpt, DRAFT_ENDING_RULES);
    match analysis.scene_map.dominant_role.as_str() {
        "battle" => {
            function_counter.add_n("conflict", 3);
        }
        "dialogue" => {
            function_counter.add_n("relationship", 2);
        }
        "environment" => {
            function_counter.add_n("movement", 1);
            function_counter.add_n("procedure", 1);
        }
        "action" => {
            function_counter.add_n("movement", 2);
        }
        "mixed" => {
            function_counter.add_n("exposition", 1);
        }
        _ => {}
    }
    let sequence_count = analysis.battle_profile.sequence_count;
    if sequence_count >= 1 {
        function_counter.add_n("conflict", sequence_count);
    }
    let shift_count = analysis.dialogue_emotions.shift_count;
    if shift_count >= 1 {
        function_counter.add_n("relationship", 1);
    }
    let tracked_term_hit = analysis
        .patterns
        .iter()
        .any(|term| term.count > 0 && (term.name == "线索面板词" || term.name == "Pi竖线状态栏"));
    if analysis.tracked_term_window_count >= 1 || tracked_term_hit {
        function_counter.add_n("investigation", 1);
    }
    if analysis.ending.warn {
        ending_counter.add_n("foreshadow_flash", 1);
    }
    if shift_count >= 1 {
        ending_counter.add_n("relationship_turn", 1);
    }
    if sequence_count >= 1 {
        ending_counter.add_n("action_aftershock", 1);
        ending_counter.add_n("external_threat", 1);
    }
    Ok(DraftSignals {
        chapter_function: pick_top_label(&function_counter, "unclear"),
        ending_function: pick_top_label(&ending_counter, "unclear"),
        chapter_counter: function_counter,
        ending_counter,
    })
}

/// 对齐状态分类（对应 `classify_alignment_status` 返回值）。
#[derive(Debug, Clone, Default)]
pub struct AlignmentStatus {
    pub alignment_status: String,
    pub recommended_action: String,
    pub drift_types: Vec<String>,
    pub review_note: String,
}

/// `classify_alignment_status`：判断失配该改正文、回修施工图还是双向比对。
#[must_use]
pub fn classify_alignment_status(
    plan_chapter: &str,
    draft_chapter: &str,
    plan_ending: &str,
    draft_ending: &str,
    chapter_match: bool,
    ending_match: bool,
) -> AlignmentStatus {
    let unknown = |label: &str| label == "missing" || label == "unclear";
    let mut mismatches: Vec<String> = Vec::new();
    if !chapter_match {
        mismatches.push("chapter_function".to_string());
    }
    if !ending_match {
        mismatches.push("ending_function".to_string());
    }

    if mismatches.is_empty() {
        return AlignmentStatus {
            alignment_status: "implemented".to_string(),
            recommended_action: "retain_alignment".to_string(),
            drift_types: Vec::new(),
            review_note: "正文功能和章末收束都已接住施工图，优先只做正文局部润色。".to_string(),
        };
    }

    if (unknown(plan_chapter) || unknown(plan_ending))
        && (!unknown(draft_chapter) || !unknown(draft_ending))
    {
        return AlignmentStatus {
            alignment_status: "plan_needs_update".to_string(),
            recommended_action: "update_chapter_plan".to_string(),
            drift_types: mismatches,
            review_note: "施工图功能含糊或缺失，而正文已有较明确落点；优先判断正文新增是否有效，有效则回修 chapter-plan。".to_string(),
        };
    }

    if (unknown(draft_chapter) || unknown(draft_ending))
        && (!unknown(plan_chapter) || !unknown(plan_ending))
    {
        return AlignmentStatus {
            alignment_status: "missing_anchor".to_string(),
            recommended_action: "revise_draft".to_string(),
            drift_types: mismatches,
            review_note: "施工图有明确功能，但正文没有接出足够清晰的功能锚点；优先回修正文落点。"
                .to_string(),
        };
    }

    if mismatches.len() == 1 {
        return AlignmentStatus {
            alignment_status: "weak_implementation".to_string(),
            recommended_action: "revise_draft_or_plan_section".to_string(),
            drift_types: mismatches,
            review_note:
                "正文只接住一部分施工图；先看偏移处是否更好，再决定改正文还是局部回修 plan。"
                    .to_string(),
        };
    }

    AlignmentStatus {
        alignment_status: "draft_drift".to_string(),
        recommended_action: "compare_plan_and_draft".to_string(),
        drift_types: mismatches,
        review_note: "正文主功能和章末收束都偏离施工图；不要只修句子，先重判本章结构目标。"
            .to_string(),
    }
}

/// plan-draft 对齐快照（字段形状跟随 `build_plan_draft_alignment` 返回值；
/// 不可用时未填键为 `None`，渲染侧按键取默认值）。
#[derive(Debug, Clone, Default)]
pub struct Alignment {
    pub available: bool,
    /// `"chapter_plan_not_resolved"` / `"novel_dir_not_resolved"`（未提供时 `None`）。
    pub reason: Option<String>,
    pub novel_dir: Option<PathBuf>,
    pub chapter_plan_path: Option<PathBuf>,
    pub plan_chapter_function: Option<String>,
    pub draft_chapter_function: Option<String>,
    pub plan_ending_function: Option<String>,
    pub draft_ending_function: Option<String>,
    pub chapter_match: Option<bool>,
    pub ending_match: Option<bool>,
    pub mismatch_count: Option<usize>,
    pub alignment_status: Option<String>,
    pub recommended_action: Option<String>,
    pub drift_types: Option<Vec<String>>,
    pub review_note: Option<String>,
    pub chapter_counter: Option<Counter>,
    pub ending_counter: Option<Counter>,
}

impl Alignment {
    /// story summary 的 `novel_dir is None` 分支：`{"available": False}`（无 reason 键）。
    #[must_use]
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// 报告侧 `novel_dir` 未解析：`{"available": False, "reason": "novel_dir_not_resolved"}`。
    #[must_use]
    pub fn novel_dir_not_resolved(novel_dir: Option<PathBuf>) -> Self {
        Self {
            reason: Some("novel_dir_not_resolved".to_string()),
            novel_dir,
            ..Self::default()
        }
    }
}

/// `build_plan_draft_alignment`：施工图 × 草稿双侧信号合成对齐快照。
pub fn build_plan_draft_alignment(
    engine: &PlanEngine,
    draft_path: &Path,
    novel_dir: Option<&Path>,
    analysis: &Analysis,
) -> Result<Alignment> {
    let Some(novel_dir) = novel_dir else {
        return Ok(Alignment::novel_dir_not_resolved(None));
    };
    let chapter_plan_path = match chapter_plan_path_for_draft(draft_path, novel_dir, engine.paths())
    {
        Some(path) => path,
        None => {
            return Ok(Alignment {
                reason: Some("chapter_plan_not_resolved".to_string()),
                novel_dir: Some(novel_dir.to_path_buf()),
                ..Alignment::default()
            });
        }
    };
    let plan_signals = collect_plan_signals(engine, &chapter_plan_path)?;
    let draft_signals = infer_draft_signals(draft_path, analysis)?;
    let chapter_match = plan_signals.chapter_function == draft_signals.chapter_function;
    let ending_match = plan_signals.ending_function == draft_signals.ending_function;
    let status = classify_alignment_status(
        &plan_signals.chapter_function,
        &draft_signals.chapter_function,
        &plan_signals.ending_function,
        &draft_signals.ending_function,
        chapter_match,
        ending_match,
    );
    Ok(Alignment {
        available: plan_signals.available,
        reason: None,
        novel_dir: Some(novel_dir.to_path_buf()),
        chapter_plan_path: Some(chapter_plan_path.clone()),
        plan_chapter_function: Some(plan_signals.chapter_function),
        draft_chapter_function: Some(draft_signals.chapter_function),
        plan_ending_function: Some(plan_signals.ending_function),
        draft_ending_function: Some(draft_signals.ending_function),
        chapter_match: Some(chapter_match),
        ending_match: Some(ending_match),
        mismatch_count: Some(usize::from(!chapter_match) + usize::from(!ending_match)),
        alignment_status: Some(status.alignment_status),
        recommended_action: Some(status.recommended_action),
        drift_types: Some(status.drift_types),
        review_note: Some(status.review_note),
        chapter_counter: Some(draft_signals.chapter_counter),
        ending_counter: Some(draft_signals.ending_counter),
    })
}
