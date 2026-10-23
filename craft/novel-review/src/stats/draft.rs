//! `stats.draft`：章节级报告、滚动窗口合并报告与按目录分组的 SUMMARY，输出为镜像 markdown 树。
//!
//! - 章节/窗口分析全部走 `audit::draft` 的 `analyze_path`/`analyze_text`；
//! - 语料学习：缺省开启（`corpus_paths_for_targets`），`--no-corpus-learning` 禁用；
//! - 输出路径：`--output-root` 下按 novel 名镜像 `draft-stats/` 树；
//!   缺省时写回输入树（`drafts/` → `draft-stats/`）。

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use fancy_regex::Regex;

use crate::audit::draft::{
    analyze_path, analyze_text, build_corpus_profile, format_markdown_report, iter_target_files,
    Analysis, CorpusProfile, DraftContext,
};
use crate::config::{self, EndingLabels, TemplateRule, TrackedTerm};
use crate::input::{resolve_inputs, write_text};
use crate::rules::build_template_bank;

/// 章节文件名匹配（忽略大小写 `ch<数字>` 正则）。
static CHAPTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)ch(\\d+)").expect("CHAPTER_RE 应可编译"));

/// `stats.draft` 子命令参数。
#[derive(Debug, Clone)]
pub struct StatsDraftOptions {
    /// 位置参数：草稿文件或目录。
    pub positional: Vec<PathBuf>,
    /// 规则文件路径；缺省为 `configs/rules/review.yaml`。
    pub rules: Option<PathBuf>,
    /// `-i/--input`：输入草稿文件或目录（可重复）。
    pub inputs: Vec<PathBuf>,
    /// `-o/--output`：单文件章节报告输出（仅当只收集到一个章节时允许）。
    pub output: Option<PathBuf>,
    /// `--output-root`：生成 stats 的镜像树根（缺省时写回 `draft-stats/` 镜像树）。
    pub output_root: Option<PathBuf>,
    /// 每条规则最多记录的样本行数（默认 3）。
    pub sample_limit: usize,
    /// 滚动章节窗口大小（默认 `[2, 3]`）。
    pub window_sizes: Vec<usize>,
    /// `--no-corpus-learning`：禁用从既有卡片/计划/草稿学到的筛选器。
    pub no_corpus_learning: bool,
}

/// 章节分析结果对（`lib.analysis.analyze_files` 的移植）。
pub type ChapterAnalysis = (PathBuf, Analysis);

/// 章末标签展示名（缺省为 label 本身，查表）。
pub fn ending_display(labels: &EndingLabels, label: &str) -> String {
    labels
        .display
        .get(label)
        .cloned()
        .unwrap_or_else(|| label.to_string())
}
/// 章排序键：章号优先，无章号排 9999。
#[must_use]
pub fn chapter_sort_key(path: &Path) -> (i64, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let number = match CHAPTER_RE.captures(&stem) {
        Ok(Some(caps)) => caps
            .get(1)
            .map(|g| g.as_str())
            .and_then(|s| s.parse::<i64>().ok()),
        _ => None,
    };
    match number {
        Some(n) => (n, name),
        None => (9999, name),
    }
}

/// `lib.paths.collect_chapter_files`：展开输入后过滤章节文件，
/// 按 `(父目录字符串, chapter_sort_key)` 稳定排序。
pub fn collect_chapter_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let files = iter_target_files(paths);
    let mut chapter_files: Vec<PathBuf> = Vec::new();
    for path in files {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if CHAPTER_RE.is_match(&stem).unwrap_or(false) {
            chapter_files.push(path);
        }
    }
    chapter_files.sort_by(|a, b| {
        let key = |p: &PathBuf| {
            (
                p.parent()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                chapter_sort_key(p),
            )
        };
        key(a).cmp(&key(b))
    });
    Ok(chapter_files)
}

/// 由路径 components 重组路径（`"/"` 组件还原为绝对根）。
pub fn path_from_parts(parts: &[String]) -> PathBuf {
    let mut out = PathBuf::new();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 && part == "/" {
            out.push("/");
        } else {
            out.push(part);
        }
    }
    out
}

/// `stats/draft.stats_path_for`：镜像树报告路径。
///
/// 有 `output_root` 时镜像到 `output_root/{novel}/draft-stats/...`；
/// 缺省返回输入路径中正文目录名 → `draft-stats/` 的替换。正文目录名来自
/// `paths` 配置，不是内置常量。
pub fn stats_path_for(
    draft_path: &Path,
    output_root: Option<&Path>,
    paths: &crate::config::PathsConfig,
) -> Result<PathBuf> {
    let parts: Vec<String> = draft_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let idx = paths
        .drafts_dir_component(draft_path)
        .with_context(|| format!("Path does not live under drafts/: {}", draft_path.display()))?;

    let mut stats_parts = parts.clone();
    stats_parts[idx] = "draft-stats".to_string();

    let Some(out_root) = output_root else {
        return Ok(path_from_parts(&stats_parts));
    };

    // stats_path.relative_to(novel_root) = parts[idx..]（draft-stats 起）
    let tail: Vec<&str> = stats_parts[idx..].iter().map(|s| s.as_str()).collect();
    let relative = tail.join("/");
    if idx == 0 {
        Ok(out_root.join(relative))
    } else {
        let novel_name = &parts[idx - 1];
        Ok(out_root.join(novel_name).join(relative))
    }
}

/// `infer_ending_label`：从章末节的 tail/flow/image 词项推断收束标签。
pub fn infer_ending_label(analysis: &Analysis, labels: &EndingLabels) -> String {
    let ending = &analysis.ending;
    let flow_terms = ending
        .flow_terms
        .iter()
        .map(|t| t.term.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let image_terms = ending
        .image_terms
        .iter()
        .map(|t| t.term.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("{} {} {}", ending.tail_excerpt, flow_terms, image_terms);

    // scores[label] = rules 词项 substring 命中和（赋值），
    // 再对 imagery_coda / procedure_pressure 按词项数累加进同一 key（Counter 语义），
    // 最后 (-count, label) 稳定排序，并列判 mixed。
    let mut scores: std::collections::HashMap<String, usize> = labels
        .rules
        .iter()
        .map(|(label, terms)| {
            (
                label.clone(),
                terms
                    .iter()
                    .map(|t| text.matches(t.as_str()).count())
                    .sum::<usize>(),
            )
        })
        .collect();
    if !image_terms.trim().is_empty() {
        *scores.entry("imagery_coda".to_string()).or_insert(0) += ending.image_terms.len();
    }
    if !flow_terms.trim().is_empty() {
        *scores.entry("procedure_pressure".to_string()).or_insert(0) += ending.flow_terms.len();
    }
    let mut ranked: Vec<(String, usize)> = scores.into_iter().filter(|(_, c)| *c > 0).collect();
    if ranked.is_empty() {
        return "unclear".to_string();
    }
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (top_label, top_count) = &ranked[0];
    if ranked.len() > 1 && ranked[1].1 == *top_count && &ranked[1].0 != top_label {
        return "mixed".to_string();
    }
    top_label.clone()
}

/// `summarize_runs`：连续同标签收成 `展示名 x计数` 序列（缺省 min_run=2、limit=4）。
pub fn summarize_runs(
    labels_in_order: &[String],
    min_run: usize,
    limit: usize,
    labels: &EndingLabels,
) -> Vec<String> {
    if labels_in_order.is_empty() {
        return Vec::new();
    }
    let mut runs: Vec<String> = Vec::new();
    let mut current = &labels_in_order[0];
    let mut count = 1usize;
    for label in &labels_in_order[1..] {
        if label == current {
            count += 1;
            continue;
        }
        if count >= min_run {
            runs.push(format!("{} x{}", ending_display(labels, current), count));
        }
        current = label;
        count = 1;
    }
    if count >= min_run {
        runs.push(format!("{} x{}", ending_display(labels, current), count));
    }
    runs.truncate(limit);
    runs
}

/// `ending_flow_text`：按序标签 → `A -> B -> C`（缺省 limit=8，空为「无」）。
pub fn ending_flow_text(labels_in_order: &[String], limit: usize, labels: &EndingLabels) -> String {
    if labels_in_order.is_empty() {
        return "无".to_string();
    }
    labels_in_order
        .iter()
        .take(limit)
        .map(|l| ending_display(labels, l))
        .collect::<Vec<_>>()
        .join(" -> ")
}

/// 分析环境：共享 `ctx` / 模板库 / 词库 / 语料画像 / 采样上限，
pub struct AnalysisEnv<'a> {
    /// 草稿分析上下文（规则节 + 编译正则 + 分词器）。
    pub ctx: &'a DraftContext,
    /// `build_template_bank` 结果（`draft.regex_rules` + 模板句）。
    pub template_bank: &'a [TemplateRule],
    /// `draft.tracked_terms` 词库。
    pub term_bank: &'a [TrackedTerm],
    /// 语料学习画像（`--no-corpus-learning` 时为 None）。
    pub corpus_profile: Option<&'a CorpusProfile>,
    /// 每条规则最多记录的样本行数。
    pub sample_limit: usize,
}

impl AnalysisEnv<'_> {
    /// `analyze_chapters`（`lib.analysis.analyze_files` 移植）：逐章分析。
    pub fn analyze_chapters(&self, files: &[PathBuf]) -> Result<Vec<ChapterAnalysis>> {
        let mut out: Vec<ChapterAnalysis> = Vec::new();
        for path in files {
            let analysis = analyze_path(
                self.ctx,
                path,
                self.template_bank,
                self.term_bank,
                self.corpus_profile,
                self.sample_limit,
            )
            .with_context(|| format!("无法分析章节 {}", path.display()))?;
            out.push((path.clone(), analysis));
        }
        Ok(out)
    }

    /// `analyze_paths`：多章合并为单一 analysis（source 为 ` | ` 连接）。
    fn analyze_paths(&self, paths: &[PathBuf]) -> Result<Analysis> {
        let combined_text = paths
            .iter()
            .map(|p| {
                std::fs::read_to_string(p).with_context(|| format!("无法读取文件 {}", p.display()))
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n\n");
        let source = paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ");
        analyze_text(
            self.ctx,
            &combined_text,
            &source,
            self.template_bank,
            self.term_bank,
            self.corpus_profile,
            self.sample_limit,
        )
    }
}

/// 单文件章节报告。
pub fn build_single_reports(
    env: &AnalysisEnv,
    files: &[PathBuf],
    output_root: Option<&Path>,
    chapter_analyses: Option<&[ChapterAnalysis]>,
    single_output: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let analyses = match chapter_analyses {
        Some(a) => a.to_vec(),
        None => env.analyze_chapters(files)?,
    };
    let layout = &env.ctx.paths();
    let mut written: Vec<PathBuf> = Vec::new();
    for (draft_path, analysis) in analyses {
        let title = draft_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let report = format_markdown_report(&analysis, Some(&title));
        let out_path = match single_output {
            Some(path) => path.to_path_buf(),
            None => stats_path_for(&draft_path, output_root, layout)?,
        };
        let content = if report.ends_with('\n') {
            report.clone()
        } else {
            format!("{report}\n")
        };
        write_text(&out_path, &content)?;
        written.push(out_path);
    }
    Ok(written)
}

/// 滚动窗口合并报告。
fn build_window_report(
    env: &AnalysisEnv,
    paths: &[PathBuf],
    window_name: &str,
    output_root: Option<&Path>,
    analysis: Option<&Analysis>,
) -> Result<(PathBuf, String, Analysis)> {
    let first_stem = paths
        .first()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_stem = paths
        .last()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = format!("{first_stem}-{last_stem}");
    let window_analysis = match analysis {
        Some(a) => a.clone(),
        None => env.analyze_paths(paths)?,
    };
    let layout = &env.ctx.paths();
    let parent_stats_dir =
        stats_path_for(paths.first().expect("窗口路径非空"), output_root, layout)?
            .parent()
            .map(|p| p.to_path_buf())
            .with_context(|| "stats 路径无父目录")?;
    let out_path = parent_stats_dir
        .join(window_name)
        .join(format!("{title}.md"));
    let report = format_markdown_report(&window_analysis, Some(&title));
    Ok((out_path, report, window_analysis))
}

/// Counter（首现序 + 计数；most_common 平手按首现序）。
///
/// - `add`：新 key 记入首现序；计数累加。
/// - `items`：**首现插入序** + 当前计数。
/// - `most_common`/`most_common_all`：**只按 count 稳定降序**，并列保持首次出现序
///   （**不可**加 key 二次序）。
/// - `get`/`count`：缺省 0；`is_empty`：无 key 即空（真值判断）。
#[derive(Debug, Default)]
pub struct Ctr {
    order: Vec<String>,
    map: std::collections::HashMap<String, usize>,
}

impl Ctr {
    /// `Counter.__init__/+=` 语义：新 key 记入首现序；计数累加。
    pub fn add(&mut self, key: &str, n: usize) {
        if !self.map.contains_key(key) {
            self.order.push(key.to_string());
        }
        *self.map.entry(key.to_string()).or_insert(0) += n;
    }
    /// `Counter.most_common(n)`：计数降序；并列 = 首次出现序。
    pub fn most_common(&self, n: usize) -> Vec<(String, usize)> {
        self.most_common_all().into_iter().take(n).collect()
    }
    /// `Counter.most_common()`：全部条目（计数降序；并列 = 首次出现序）。
    pub fn most_common_all(&self) -> Vec<(String, usize)> {
        let mut items: Vec<(String, usize)> = self
            .order
            .iter()
            .map(|k| (k.clone(), self.map.get(k).copied().unwrap_or(0)))
            .collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.1));
        items
    }
    /// `counter.items()` 遍历序：**首现插入序** + 当前计数（不做 count 排序）。
    pub fn items(&self) -> Vec<(String, usize)> {
        self.order
            .iter()
            .map(|k| (k.clone(), self.map.get(k).copied().unwrap_or(0)))
            .collect()
    }
    /// `Counter[key]` / `Counter.get(key, 0)`：无则 0。
    pub fn get(&self, key: &str) -> usize {
        self.map.get(key).copied().unwrap_or(0)
    }
    /// `Counter.__getitem__`：缺省 0。
    pub fn count(&self, key: &str) -> usize {
        self.map.get(key).copied().unwrap_or(0)
    }
    /// 真值判断。
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// 按父目录分组的 SUMMARY + 滚动窗口。
pub fn build_group_reports(
    env: &AnalysisEnv,
    files: &[PathBuf],
    window_sizes: &[usize],
    output_root: Option<&Path>,
    chapter_analyses: Option<&[ChapterAnalysis]>,
    labels: &EndingLabels,
) -> Result<Vec<PathBuf>> {
    let mut written: Vec<PathBuf> = Vec::new();
    // 按父目录分组（首现序）
    let mut groups: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for path in files {
        let parent = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        if let Some(g) = groups.iter_mut().find(|g| g.0 == parent) {
            g.1.push(path.clone());
        } else {
            groups.push((parent, vec![path.clone()]));
        }
    }

    let analyses = match chapter_analyses {
        Some(a) => a.to_vec(),
        None => env.analyze_chapters(files)?,
    };
    let layout = &env.ctx.paths();

    for group_files in groups.iter().map(|g| &g.1) {
        let mut ordered = group_files.clone();
        ordered.sort_by(|a, b| {
            let (ka, kb) = (chapter_sort_key(a), chapter_sort_key(b));
            ka.cmp(&kb)
        });
        let chapter_analyses: Vec<(PathBuf, Analysis)> = ordered
            .iter()
            .map(|item| {
                let analysis = analyses
                    .iter()
                    .find(|(p, _)| p == item)
                    .expect("章节分析已按同一文件列表构建")
                    .1
                    .clone();
                (item.clone(), analysis)
            })
            .collect();

        let mut summary_lines: Vec<String> =
            vec!["# SUMMARY".into(), String::new(), "## Chapters".into()];
        for (item, analysis) in &chapter_analyses {
            summary_lines.push(format!(
                "- `{}` status=`{}` warn_sections=`{}` chars=`{}`",
                item.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                if analysis.warned { "WARN" } else { "OK" },
                analysis.summary.warn_sections,
                analysis.summary.chars
            ));
        }
        summary_lines.push(String::new());

        let mut hard_flag_counter = Ctr::default();
        let mut style_fatigue_counter = Ctr::default();
        let mut sentence_pattern_counter = Ctr::default();
        let mut short_phrase_counter = Ctr::default();
        let mut term_counter = Ctr::default();
        let mut ending_label_counter = Ctr::default();
        let mut ending_labels_in_order: Vec<String> = Vec::new();

        for (_item, analysis) in &chapter_analyses {
            for flag in &analysis.hard_flags {
                hard_flag_counter.add(&format!("{}|{}", flag.section, flag.name), flag.count);
            }
            for item in analysis.style_fatigue.iter().filter(|i| i.status == "WARN") {
                style_fatigue_counter.add(&format!("{}|{}", item.status, item.family), item.count);
            }
            for pattern in &analysis.sentence_patterns {
                sentence_pattern_counter.add(&pattern.phrase, pattern.count);
            }
            for phrase in analysis.short_phrases.iter().take(20) {
                short_phrase_counter.add(&phrase.term, phrase.count);
            }
            for term in analysis.terms.iter().take(20) {
                term_counter.add(&term.term, term.count);
            }
            let ending_label = infer_ending_label(analysis, labels);
            ending_label_counter.add(&ending_label, 1);
            ending_labels_in_order.push(ending_label);
        }

        let render_pairs = |counter: &Ctr, limit: usize, line: &mut Vec<String>| {
            if counter.order.is_empty() {
                line.push("- 无".into());
            } else {
                for (key, count) in counter.most_common(limit) {
                    let (l, r) = key.split_once('|').unwrap_or((&key, ""));
                    if l.is_empty() && r.is_empty() {
                        line.push(format!("- `{key}` total=`{count}`"));
                    } else {
                        line.push(format!("- `{l}` `{r}` total=`{count}`"));
                    }
                }
            }
            line.push(String::new());
        };

        summary_lines.push("## Story-Wide Hard Flags".into());
        render_pairs(&hard_flag_counter, 15, &mut summary_lines);
        summary_lines.push("## Story-Wide Style Fatigue".into());
        render_pairs(&style_fatigue_counter, 15, &mut summary_lines);
        summary_lines.push("## Story-Wide Sentence Skeletons".into());
        {
            if sentence_pattern_counter.order.is_empty() {
                summary_lines.push("- 无".into());
            } else {
                for (name, count) in sentence_pattern_counter.most_common(12) {
                    summary_lines.push(format!("- `{name}` total=`{count}`"));
                }
            }
            summary_lines.push(String::new());
        }
        summary_lines.push("## Story-Wide Structural Phrases".into());
        {
            if short_phrase_counter.order.is_empty() {
                summary_lines.push("- 无".into());
            } else {
                for (name, count) in short_phrase_counter.most_common(15) {
                    summary_lines.push(format!("- `{name}` total=`{count}`"));
                }
            }
            summary_lines.push(String::new());
        }
        summary_lines.push("## Story-Wide Repeated Terms".into());
        {
            if term_counter.order.is_empty() {
                summary_lines.push("- 无".into());
            } else {
                for (name, count) in term_counter.most_common(15) {
                    summary_lines.push(format!("- `{name}` total=`{count}`"));
                }
            }
            summary_lines.push(String::new());
        }

        summary_lines.push("## Story-Wide Ending Functions".into());
        if ending_label_counter.order.is_empty() {
            summary_lines.push("- 无".into());
        } else {
            for (label, count) in ending_label_counter.most_common(10) {
                summary_lines.push(format!(
                    "- `{}` total=`{count}`",
                    ending_display(labels, &label)
                ));
            }
            summary_lines.push(format!(
                "- flow=`{}`",
                ending_flow_text(&ending_labels_in_order, 8, labels)
            ));
            let runs = summarize_runs(&ending_labels_in_order, 2, 4, labels);
            summary_lines.push(format!(
                "- repeated=`{}`",
                if runs.is_empty() {
                    "无".to_string()
                } else {
                    runs.join(" | ")
                }
            ));
        }
        summary_lines.push(String::new());

        summary_lines.push("## Priority".into());
        let mut priority_sorted = chapter_analyses.clone();
        priority_sorted.sort_by(|a, b| {
            b.1.summary
                .warn_sections
                .cmp(&a.1.summary.warn_sections)
                .then_with(|| b.1.summary.chars.cmp(&a.1.summary.chars))
                .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
        });
        for (item, analysis) in priority_sorted.iter().take(5) {
            let top_templates = if analysis.template_candidates.is_empty() {
                "无".to_string()
            } else {
                analysis
                    .template_candidates
                    .iter()
                    .take(3)
                    .map(|c| format!("{} x{}", c.name, c.count))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let top_fatigue = {
                let parts: Vec<String> = analysis
                    .style_fatigue
                    .iter()
                    .filter(|f| f.status == "WARN")
                    .map(|f| format!("{} x{}", f.family, f.count))
                    .collect();
                if parts.is_empty() {
                    "无".to_string()
                } else {
                    parts.join(", ")
                }
            };
            let ending_label = ending_display(labels, &infer_ending_label(analysis, labels));
            summary_lines.push(format!(
                "- `{}` warn_sections=`{}` ending=`{ending_label}` top=`{top_templates}` fatigue=`{top_fatigue}`",
                item.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                analysis.summary.warn_sections
            ));
        }
        summary_lines.push(String::new());

        for size in window_sizes {
            let window_name = match size {
                2 => "pairs".to_string(),
                3 => "triples".to_string(),
                _ => format!("window-{size}"),
            };
            summary_lines.push(format!("## {window_name}"));
            let mut built_any = false;
            let mut window_summaries: Vec<(String, Analysis, Vec<String>)> = Vec::new();
            let count = ordered.len() as i64 - *size as i64 + 1;
            for idx in 0..count.max(0) {
                let start = idx as usize;
                let chunk: Vec<PathBuf> = ordered[start..(start + *size)].to_vec();
                let (out_path, report, analysis) =
                    build_window_report(env, &chunk, &window_name, output_root, None)?;
                let chunk_labels: Vec<String> = chapter_analyses
                    .iter()
                    .filter(|(p, _)| chunk.iter().any(|c| c == p))
                    .map(|(_, a)| infer_ending_label(a, labels))
                    .collect();
                let content = if report.ends_with('\n') {
                    report.clone()
                } else {
                    format!("{report}\n")
                };
                write_text(&out_path, &content)?;
                written.push(out_path.clone());
                summary_lines.push(format!(
                    "- `{}` status=`{}` warn_sections=`{}` chars=`{}` endings=`{}`",
                    out_path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    if analysis.warned { "WARN" } else { "OK" },
                    analysis.summary.warn_sections,
                    analysis.summary.chars,
                    ending_flow_text(&chunk_labels, *size, labels)
                ));
                window_summaries.push((
                    out_path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    analysis,
                    chunk_labels,
                ));
                built_any = true;
            }
            if !built_any {
                summary_lines.push("- 无".into());
            }
            summary_lines.push(String::new());
            if built_any {
                summary_lines.push(format!("### {window_name}-priority"));
                let mut wsorted = window_summaries.clone();
                wsorted.sort_by(|a, b| {
                    b.1.summary
                        .warn_sections
                        .cmp(&a.1.summary.warn_sections)
                        .then_with(|| b.1.summary.chars.cmp(&a.1.summary.chars))
                        .then_with(|| a.0.cmp(&b.0))
                });
                for (name, analysis, chunk_labels) in wsorted.iter().take(3) {
                    let top_templates = if analysis.template_candidates.is_empty() {
                        "无".to_string()
                    } else {
                        analysis
                            .template_candidates
                            .iter()
                            .take(3)
                            .map(|c| format!("{} x{}", c.name, c.count))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    let top_fatigue = {
                        let parts: Vec<String> = analysis
                            .style_fatigue
                            .iter()
                            .filter(|f| f.status == "WARN")
                            .map(|f| format!("{} x{}", f.family, f.count))
                            .collect();
                        if parts.is_empty() {
                            "无".to_string()
                        } else {
                            parts.join(", ")
                        }
                    };
                    let ending_runs = summarize_runs(chunk_labels, 2, 4, labels);
                    summary_lines.push(format!(
                        "- `{name}` warn_sections=`{}` endings=`{}` repeated=`{}` top=`{top_templates}` fatigue=`{top_fatigue}`",
                        analysis.summary.warn_sections,
                        ending_flow_text(chunk_labels, *size, labels),
                        if ending_runs.is_empty() {
                            "无".to_string()
                        } else {
                            ending_runs.join(" | ")
                        }
                    ));
                }
                summary_lines.push(String::new());
            }
        }

        let first = ordered.first().expect("分组非空");
        let summary_path = stats_path_for(first, output_root, layout)?
            .parent()
            .map(|p| p.to_path_buf())
            .with_context(|| "stats 路径无父目录")?
            .join("SUMMARY.md");
        write_text(&summary_path, &format!("{}\n", summary_lines.join("\n")))?;
        written.push(summary_path);
    }
    Ok(written)
}

/// 收集章节、语料学习、写单文件/镜像/分组报告。
pub fn run(opts: &StatsDraftOptions) -> Result<i32> {
    if opts.output.is_some() && opts.output_root.is_some() {
        eprintln!("Use either --output or --output-root, not both.");
        return Ok(1);
    }
    let inputs = match resolve_inputs(&opts.positional, &opts.inputs) {
        Ok(inputs) => inputs,
        Err(err) => {
            eprintln!("{err}");
            return Ok(1);
        }
    };
    let files = collect_chapter_files(&inputs)?;
    if files.is_empty() {
        eprintln!("No draft files found.");
        return Ok(1);
    }
    let rules = config::load_rules_from(opts.rules.as_deref())?;
    let ctx = DraftContext::new(rules)?;
    let template_bank = build_template_bank(ctx.draft_rules());

    let corpus_profile: Option<CorpusProfile> = if opts.no_corpus_learning {
        None
    } else {
        let corpus_paths = ctx.corpus_paths_for_targets(&files);
        build_corpus_profile(&ctx, &corpus_paths, &files)?
    };

    let env = AnalysisEnv {
        ctx: &ctx,
        template_bank: &template_bank,
        term_bank: &ctx.draft_rules().tracked_terms,
        corpus_profile: corpus_profile.as_ref(),
        sample_limit: opts.sample_limit,
    };

    let chapter_analyses = env.analyze_chapters(&files)?;

    if let Some(output) = &opts.output {
        if files.len() != 1 {
            eprintln!("--output requires exactly one collected draft chapter.");
            return Ok(1);
        }
        build_single_reports(&env, &files, None, Some(&chapter_analyses), Some(output))?;
        return Ok(0);
    }

    let output_root = opts.output_root.as_deref();
    build_single_reports(&env, &files, output_root, Some(&chapter_analyses), None)?;
    let labels = &ctx.draft_rules().ending_labels;
    build_group_reports(
        &env,
        &files,
        &opts.window_sizes,
        output_root,
        Some(&chapter_analyses),
        labels,
    )?;
    Ok(0)
}
