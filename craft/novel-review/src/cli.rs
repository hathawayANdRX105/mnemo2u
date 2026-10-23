//! The review tool's CLI: argument parsing and dispatch.
//!
//! Keeping the dispatch as a function rather than in `main` is what lets a
//! thin wrapper binary host the same commands, and lets tests drive it
//! without going through a process boundary.
//!
//! 子命令：`rules`（配置校验统计）、`audit-draft`（草稿全量分析；
//! `--format json` 输出遵循固定 JSON 契约，text/markdown 渲染逐字节稳定）、
//! `audit-plan` / `audit-concept`（大纲与概念卡审查）、
//! `stats-draft`（章节/滚动窗口镜像统计树）与 `stats-plan` / `stats-concept`
//! （镜像 markdown 统计树）、
//! `consistency`（SQLite/FTS5 一致性索引与 13 个子命令）。

use std::path::PathBuf;
use std::process;

use crate::{audit, config, consistency, reports, stats, study, style, tools};
use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

/// 小说大纲/草稿审查与统计工具。
#[derive(Parser)]
#[command(name = "novel-review", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// `--format` 报告格式取值（audit-plan）。
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ReportFormat {
    /// 文本（markdown 报告，stdout 或 .txt 文件）
    Text,
    /// JSON 报告
    Json,
    /// Markdown 报告（.md 文件）
    Markdown,
}

#[derive(Subcommand)]
pub enum Command {
    /// 加载并校验 review.yaml，输出各节统计
    Rules {
        /// 规则文件路径（默认 configs/rules/review.yaml）
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 分析草稿：全量指标、场面/对白/语料学习（`--format json` 输出遵循固定 JSON 契约）
    AuditDraft {
        /// 草稿文件或目录（多文件输出 JSON 数组）
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 输入文件或目录（可重复）
        #[arg(short = 'i', long, value_name = "PATH")]
        input: Vec<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 3)]
        sample_limit: usize,
        /// 有警告时以退出码 1 结束
        #[arg(long)]
        fail_on_warn: bool,
        /// 报告输出格式（三种格式输出逐字节稳定）
        #[arg(long, value_enum, default_value_t = ReportFormat::Text)]
        format: ReportFormat,
        /// 输出文件（json）
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// 语料学习路径（可多个；缺省时自动定位同小说的 concept/cards、plans、drafts）
        #[arg(long, value_name = "PATH", num_args = 0..)]
        learn_from: Option<Vec<PathBuf>>,
        /// 禁用语料学习
        #[arg(long)]
        no_corpus_learning: bool,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 审查大纲（arc/story/chapter）plan 文件：结构漂移与字段误用
    AuditPlan {
        /// Plan 文件或目录
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 输入 plan 文件或目录（可重复）
        #[arg(short = 'i', long, value_name = "PATH")]
        input: Vec<PathBuf>,
        /// 报告输出格式
        #[arg(long, value_enum, default_value_t = ReportFormat::Markdown)]
        format: ReportFormat,
        /// 有警告时以退出码 1 结束
        #[arg(long)]
        fail_on_warn: bool,
        /// 输出文件或目录
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 审查概念卡：缺失字段与分类漂移
    AuditConcept {
        /// 概念卡文件或目录（必填）
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 把模板文件纳入审查
        #[arg(long)]
        include_templates: bool,
    },
    /// 计划镜像统计：逐文件 *-stats 报告 + 逐目录 SUMMARY.md
    StatsPlan {
        /// Plan 文件或目录
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 输入 plan 文件或目录（可重复）
        #[arg(short = 'i', long, value_name = "PATH")]
        input: Vec<PathBuf>,
        /// 单文件报告输出（仅在收集到恰好一个 plan 文件时允许）
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// 镜像统计根目录；缺省写入小说本地的 *-stats 树
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 概念卡镜像统计：card-stats 树 + 逐目录 SUMMARY.md
    StatsConcept {
        /// 概念卡文件或目录（必填）
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 把模板文件纳入审查
        #[arg(long)]
        include_templates: bool,
    },
    /// 章节镜像统计：章节报告 + 滚动窗口合并 + 逐目录 SUMMARY
    StatsDraft {
        /// 草稿文件或目录
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 输入草稿文件或目录（可重复）
        #[arg(short = 'i', long, value_name = "PATH")]
        input: Vec<PathBuf>,
        /// 单文件章节报告输出（仅在收集到恰好一个章节时允许）
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// 镜像统计根目录；缺省写入小说本地的 draft-stats 树
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 3)]
        sample_limit: usize,
        /// 滚动章节窗口大小（缺省 2 3；显式 `--window-sizes` 不带值时为空列表）
        #[arg(long, value_name = "SIZE", num_args = 0..)]
        window_sizes: Option<Vec<usize>>,
        /// 禁用从既有卡片、计划、草稿学到的筛选器
        #[arg(long)]
        no_corpus_learning: bool,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 草稿章节评审记分卡：scorecards/*.md + 逐 story SUMMARY.md
    ReportsScorecard {
        /// 草稿章节文件或目录
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 6)]
        sample_limit: usize,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在输入树旁
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 草稿章节评审学习日志：learning/*.md + 逐 story SUMMARY.md
    ReportsLearning {
        /// 草稿章节文件或目录
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 6)]
        sample_limit: usize,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在输入树旁
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 研究导向章节句子画像：profiles/*.md + 逐 story SUMMARY.md
    ReportsProfiles {
        /// 草稿章节文件或目录
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 每节最多渲染的条数
        #[arg(long, default_value_t = 8)]
        sample_limit: usize,
        /// 可选镜像根目录；缺省写小说本地的 draft-stats 树
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 跨 Story 模板/词项候选目录：draft-stats/template-catalog/{SUMMARY.md,CATALOG.json}
    ReportsCatalog {
        /// novel 目录、draft 目录或草稿章节文件
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在输入树旁
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 跨章模板积压：template-backlog/{SUMMARY.md,CANDIDATES.json}
    ReportsBacklog {
        /// 草稿章节文件或目录
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 6)]
        sample_limit: usize,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在输入树旁
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 故事级评审套件：单章三类报告 + story 级 SUMMARY + review-kit/SUMMARY.md
    ReportsKit {
        /// 草稿章节文件或目录
        #[arg(required = true, value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 6)]
        sample_limit: usize,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在输入树旁
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 记忆层：设定 / 事实 / 关系三层入 mnemo2u
    Memory {
        #[command(subcommand)]
        cmd: novel_memory::MemoryCmd,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 一致性索引：SQLite/FTS5 构建与查询
    Consistency {
        #[command(subcommand)]
        cmd: consistency::ConsistencyCmd,
    },
    /// 整工作区看板：concept/plan/draft/consistency 四节 → 单份 AUDIT.md
    ReportsWorkspace {
        /// novel 目录（例如 novel1）
        #[arg(required = true, value_name = "NOVEL_DIR")]
        novel_dir: PathBuf,
        /// 每条规则最多记录的样本行数
        #[arg(long, default_value_t = 3)]
        sample_limit: usize,
        /// 滚动章节窗口大小（缺省 2 3；显式 `--window-sizes` 不带值时为空列表）
        #[arg(long, value_name = "SIZE", num_args = 0..)]
        window_sizes: Option<Vec<usize>>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
        /// 产物根目录；缺省写在小说本地
        #[arg(long, value_name = "PATH")]
        output_root: Option<PathBuf>,
    },
    /// 模板候选回写：dry-run 预览或写回 review.yaml
    ToolsApply {
        /// 模板目录 CATALOG.json 路径
        #[arg(required = true, value_name = "CATALOG")]
        catalog: PathBuf,
        /// 只预览，不改文件
        #[arg(long)]
        dry_run: bool,
        /// 执行回写
        #[arg(long)]
        apply: bool,
        /// 要回写的规则文件；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 语义精判：读取 audit-draft JSON，按「AI 腔概率」排序命中句
    JudgeReview {
        /// audit-draft --format json 输出的 analysis JSON 文件
        #[arg(required = true, value_name = "ANALYSIS")]
        analysis: PathBuf,
        /// 输出前 N 句（默认 10）
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// 最多提交给 judge 的候选句数（noul 单批上限 64）
        #[arg(long, default_value_t = 64)]
        limit: usize,
        /// 覆盖 JUDGE_API_BASE_URL
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        /// 覆盖 JUDGE_API_KEY
        #[arg(long, value_name = "KEY")]
        api_key: Option<String>,
        /// 覆盖 JUDGE_MODEL
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// 报告写出路径（默认 stdout）
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// 以 JSON 输出结果
        #[arg(long)]
        json: bool,
        /// 调用生成模型对 top-N 逐句改写
        #[arg(long)]
        rewrite: bool,
        /// 覆盖 GATEWAY_BASE_URL（默认 http://127.0.0.1:3211/v1）
        #[arg(long, value_name = "URL")]
        rewrite_base_url: Option<String>,
        /// 覆盖 GATEWAY_API_KEY
        #[arg(long, value_name = "KEY")]
        rewrite_api_key: Option<String>,
        /// 覆盖 GATEWAY_MODEL（默认 agnes-3.0-flash）
        #[arg(long, value_name = "MODEL")]
        rewrite_model: Option<String>,
        /// 同时收集全文句子（sentence_lengths.sentences），而不只是规则命中样本
        #[arg(long)]
        all_sentences: bool,
        /// 改写后用 compare 校验语义保真，不一致则拒绝改写
        #[arg(long)]
        verify: bool,
        /// 原始草稿文件路径（配合 --output-draft 生成改写后全文）
        #[arg(long, value_name = "FILE")]
        draft: Option<PathBuf>,
        /// 改写后全文输出路径
        #[arg(long, value_name = "FILE")]
        output_draft: Option<PathBuf>,
        /// 对 top-N 句子额外做 classify 诊断分类
        #[arg(long)]
        classify: bool,
        /// 文风样本文件：改写时按样本文风特征注入匹配指导
        #[arg(long, value_name = "FILE")]
        style_sample: Option<PathBuf>,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 对比两份 analysis JSON，输出 Markdown 指标差异表
    StudyCompare {
        /// 基线 analysis JSON
        #[arg(required = true, value_name = "BASELINE")]
        baseline: PathBuf,
        /// 应用后 analysis JSON
        #[arg(required = true, value_name = "APPLIED")]
        applied: PathBuf,
    },
    /// 视角切分候选检测：逐段密度不变量 + 尾组
    StudyPov {
        /// 章节 Markdown 文件
        #[arg(required = true, value_name = "CHAPTER")]
        chapter: PathBuf,
        /// 规则文件路径；缺省为 `configs/rules/review.yaml`
        #[arg(long, value_name = "PATH")]
        rules: Option<PathBuf>,
    },
    /// 打印风格画像（注入生成 prompt 用），可带场景命中设定卡
    StyleInspect {
        /// 风格 id
        #[arg(required = true, value_name = "STYLE_ID")]
        style_id: String,
        /// 风格配置目录（默认 configs/styles）
        #[arg(long, value_name = "DIR")]
        styles_dir: Option<PathBuf>,
        /// 设定卡目录（默认 configs/cards）
        #[arg(long, value_name = "DIR")]
        cards_dir: Option<PathBuf>,
        /// 场景文本：附带该场景命中的设定卡
        #[arg(long, value_name = "TEXT")]
        scene: Option<String>,
    },
    /// 校验风格/设定卡配置（解析、id 唯一性）
    StyleCheck {
        /// 风格配置目录（默认 configs/styles）
        #[arg(long, value_name = "DIR")]
        styles_dir: Option<PathBuf>,
        /// 设定卡目录（默认 configs/cards）
        #[arg(long, value_name = "DIR")]
        cards_dir: Option<PathBuf>,
    },
    /// 文字冒险：执行一个回合（规则判定确定性，叙述可插拔）
    Adventure {
        /// 存档 JSON 文件路径
        #[arg(long, value_name = "FILE")]
        state: PathBuf,
        /// 玩家行动文本（如「攻击侍卫」「前往 雨巷」）
        #[arg(required = true, value_name = "ACTION")]
        action: String,
        /// 新建世界（覆盖已有存档）
        #[arg(long)]
        new: bool,
        /// 世界名
        #[arg(long, default_value = "无名之地", value_name = "NAME")]
        world: String,
        /// 起始场景
        #[arg(long, default_value = "起点", value_name = "TEXT")]
        scene: String,
        /// 角色名
        #[arg(long, default_value = "旅人", value_name = "NAME")]
        name: String,
        /// 角色 HP
        #[arg(long, default_value = "20", value_name = "N")]
        hp: i32,
        /// 角色攻击
        #[arg(long, default_value = "5", value_name = "N")]
        attack: i32,
        /// 角色防御
        #[arg(long, default_value = "2", value_name = "N")]
        defense: i32,
        /// 使用 LLM 叙述器（生成网关）
        #[arg(long)]
        llm: bool,
        /// 设定卡目录（动态注入场景相关设定）
        #[arg(long, value_name = "DIR")]
        cards_dir: Option<PathBuf>,
        /// 匹配设定卡用的场景文本（默认取行动文本）
        #[arg(long, value_name = "TEXT")]
        scene_text: Option<String>,
    },
}

/// Run the parsed CLI, returning the exit code the process should end with.
///
/// The caller parses and hands over the command; the actual binary is a thin
/// wrapper over this.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Rules { rules: rules_path } => {
            let path = rules_path.unwrap_or_else(config::default_rules_path);
            let cfg = config::load_rules(&path)?;
            print_rule_stats(&cfg);
        }
        Command::AuditPlan {
            paths,
            input,
            format,
            fail_on_warn,
            output,
            rules,
        } => {
            let plan_format = match format {
                ReportFormat::Text => audit::plan::OutputFormat::Text,
                ReportFormat::Json => audit::plan::OutputFormat::Json,
                ReportFormat::Markdown => audit::plan::OutputFormat::Markdown,
            };
            let rc = audit::plan::run(
                &paths,
                &input,
                plan_format,
                fail_on_warn,
                output.as_deref(),
                rules.as_deref(),
            )?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::AuditConcept {
            paths,
            include_templates,
        } => {
            let rc = audit::concept::run(&paths, include_templates)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::AuditDraft {
            paths,
            input,
            sample_limit,
            fail_on_warn,
            format,
            output,
            learn_from,
            no_corpus_learning,
            rules,
        } => {
            let draft_format = match format {
                ReportFormat::Text => audit::draft::ReportFormat::Text,
                ReportFormat::Json => audit::draft::ReportFormat::Json,
                ReportFormat::Markdown => audit::draft::ReportFormat::Markdown,
            };
            let opts = audit::draft::RunOptions {
                positional: paths,
                inputs: input,
                sample_limit,
                fail_on_warn,
                format: draft_format,
                output,
                learn_from,
                no_corpus_learning,
                rules,
            };
            let rc = audit::draft::run(&opts)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::StatsPlan {
            paths,
            input,
            output,
            output_root,
            rules,
        } => {
            let rc = stats::plan::run(
                &paths,
                &input,
                output.as_deref(),
                output_root.as_deref(),
                rules.as_deref(),
            )?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::StatsConcept {
            paths,
            include_templates,
        } => {
            let rc = stats::concept::run(&paths, include_templates)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::StatsDraft {
            paths,
            input,
            output,
            output_root,
            sample_limit,
            window_sizes,
            no_corpus_learning,
            rules,
        } => {
            let opts = stats::draft::StatsDraftOptions {
                positional: paths,
                inputs: input,
                output,
                output_root,
                sample_limit,
                window_sizes: window_sizes.unwrap_or_else(|| vec![2, 3]),
                no_corpus_learning,
                rules,
            };
            let rc = stats::draft::run(&opts)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsScorecard {
            paths,
            sample_limit,
            rules,
            output_root,
        } => {
            let opts = reports::scorecard::ScorecardOptions {
                paths,
                sample_limit,
                rules,
                output_root,
            };
            let (rc, printed) = reports::scorecard::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsLearning {
            paths,
            sample_limit,
            rules,
            output_root,
        } => {
            let opts = reports::learning::LearningOptions {
                paths,
                sample_limit,
                rules,
                output_root,
            };
            let (rc, printed) = reports::learning::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsProfiles {
            paths,
            sample_limit,
            output_root,
            rules,
        } => {
            let opts = reports::profiles::ProfileOptions {
                paths,
                sample_limit,
                output_root,
                rules,
            };
            let (rc, printed) = reports::profiles::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsCatalog {
            paths,
            rules,
            output_root,
        } => {
            let opts = reports::catalog::CatalogOptions {
                paths,
                rules,
                output_root,
            };
            let (rc, printed) = reports::catalog::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsBacklog {
            paths,
            sample_limit,
            rules,
            output_root,
        } => {
            let opts = reports::backlog::BacklogOptions {
                paths,
                sample_limit,
                rules,
                output_root,
            };
            let (rc, printed) = reports::backlog::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsKit {
            paths,
            sample_limit,
            rules,
            output_root,
        } => {
            let opts = reports::kit::KitOptions {
                paths,
                sample_limit,
                rules,
                output_root,
            };
            let (rc, printed) = reports::kit::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::Memory { cmd, rules } => {
            // 记忆层本身在 novel-memory craft 里，它不读规则文件：目录布局
            // 与记忆配置由这里从规则文件取出来传进去。
            let cfg = config::load_rules_from(rules.as_deref())?;
            let rc = novel_memory::run(&cmd, &craft_paths(&cfg.paths), &craft_config(&cfg.memory))
                .await?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::Consistency { cmd } => {
            let rc = consistency::run(&cmd)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ReportsWorkspace {
            novel_dir,
            sample_limit,
            window_sizes,
            rules,
            output_root,
        } => {
            let opts = reports::workspace::WorkspaceOptions {
                novel_dir,
                sample_limit,
                window_sizes: window_sizes.unwrap_or_else(|| vec![2, 3]),
                rules,
                output_root,
            };
            let (rc, printed) = reports::workspace::run(&opts)?;
            for path in &printed {
                println!("{}", path.display());
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::ToolsApply {
            catalog,
            dry_run,
            apply,
            rules,
        } => {
            let opts = tools::apply::ApplyOptions {
                catalog,
                dry_run,
                apply,
            };
            let (rc, dry_run_text) = tools::apply::run_with_rules(&opts, rules.as_deref())?;
            if let Some(text) = dry_run_text {
                println!("{text}");
            }
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::JudgeReview {
            analysis,
            top,
            limit,
            base_url,
            api_key,
            model,
            output,
            json,
            rewrite,
            rewrite_base_url,
            rewrite_api_key,
            rewrite_model,
            all_sentences,
            verify,
            draft,
            output_draft,
            classify,
            style_sample,
            rules,
        } => {
            let opts = tools::judge::JudgeReviewOptions {
                analysis,
                top,
                limit,
                base_url,
                api_key,
                model,
                output,
                json,
                rewrite,
                rewrite_base_url,
                rewrite_api_key,
                rewrite_model,
                all_sentences,
                verify,
                draft,
                output_draft,
                classify,
                style_sample,
                rules,
            };
            let rc = tools::judge::run(&opts)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::StudyCompare { baseline, applied } => {
            match study::compare::run_compare(&baseline, &applied) {
                Ok(table) => println!("{table}"),
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
        }
        Command::StudyPov { chapter, rules } => {
            let cfg = config::load_rules_from(rules.as_deref())?;
            match study::pov::run_pov(&chapter, &cfg) {
                Ok(json) => println!("{json}"),
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
        }
        Command::StyleInspect {
            style_id,
            styles_dir,
            cards_dir,
            scene,
        } => {
            let styles_dir = styles_dir.unwrap_or_else(style::default_styles_dir);
            let cards_dir = cards_dir.unwrap_or_else(style::default_cards_dir);
            let rc =
                style::cli::inspect(&styles_dir, Some(&cards_dir), &style_id, scene.as_deref())?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::StyleCheck {
            styles_dir,
            cards_dir,
        } => {
            let styles_dir = styles_dir.unwrap_or_else(style::default_styles_dir);
            let cards_dir = cards_dir.unwrap_or_else(style::default_cards_dir);
            let rc = style::cli::check(&styles_dir, &cards_dir)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
        Command::Adventure {
            state,
            action,
            new,
            world,
            scene,
            name,
            hp,
            attack,
            defense,
            llm,
            cards_dir,
            scene_text,
        } => {
            let opts = style::cli::AdventureOptions {
                state,
                action,
                new_world: new,
                world,
                scene,
                name,
                hp,
                attack,
                defense,
                llm,
                cards_dir,
                scene_text,
            };
            let rc = style::cli::adventure(&opts)?;
            if rc != 0 {
                process::exit(rc);
            }
        }
    }
    Ok(())
}

/// 打印规则配置各节的条目统计。
fn print_rule_stats(cfg: &config::ReviewRules) {
    let draft = &cfg.draft;
    let regex = &draft.regex_rules;
    println!("规则文件加载成功");
    println!("paths.cards: {}", cfg.paths.cards);
    println!("paths.drafts: {}", cfg.paths.drafts);
    println!("paths.arc_plan: {}", cfg.paths.arc_plan);
    println!("paths.story_plan: {}", cfg.paths.story_plan);
    println!("paths.chapter_plan: {}", cfg.paths.chapter_plan);
    println!("paths.research: {}", cfg.paths.research);
    println!("memory.embedding_model: {}", cfg.memory.embedding_model);
    println!(
        "memory.story_fact_sections: {}",
        cfg.memory.story_fact_sections.len()
    );
    println!(
        "memory.card_related_field: {}",
        cfg.memory.card_related_field
    );
    println!(
        "memory.card_relation_section: {}",
        cfg.memory.card_relation_section
    );
    println!("draft.regex_rules.tokens: {}", regex.tokens.len());
    println!("draft.regex_rules.patterns: {}", regex.patterns.len());
    println!("draft.regex_rules.phrases: {}", regex.phrases.len());
    println!("draft.regex_rules.modifiers: {}", regex.modifiers.len());
    println!("draft.regex_rules.punctuation: {}", regex.punctuation.len());
    println!(
        "draft.regex_rules.punctuation_combos: {}",
        regex.punctuation_combos.len()
    );
    println!("draft.template_rules: {}", draft.template_rules.len());
    println!(
        "draft.inactive_template_candidates: {}",
        draft.inactive_template_candidates.len()
    );
    println!("draft.tracked_terms: {}", draft.tracked_terms.len());
    println!(
        "draft.thresholds.short_sentence_max_chars: {}",
        draft.thresholds.short_sentence_max_chars
    );
    println!(
        "draft.learned_term_window.categories: {}",
        draft.learned_term_window.categories.len()
    );
    println!(
        "draft.lexicon.word_stoplist: {}",
        draft.lexicon.word_stoplist.len()
    );
    println!(
        "draft.ending_labels.display: {}",
        draft.ending_labels.display.len()
    );
    println!(
        "plan.required_headings.chapter-plan: {}",
        cfg.plan
            .required_headings
            .get("chapter-plan")
            .map_or(0, Vec::len)
    );
    println!("plan.regex: {}", cfg.plan.regex.len());
    println!(
        "plan.function_rules.chapter: {}",
        cfg.plan.function_rules.chapter.len()
    );
    println!(
        "plan.sections.chapter_function: {}",
        cfg.plan.sections.chapter_function
    );
}

/// 规则文件的 `paths` 节 → craft 的目录布局。
///
/// 两边字段一一对应，所以这里是逐字段搬过去。转换放在 CLI 而不是库里：审
/// 查逻辑不需要知道记忆层，只有这个子命令需要。
fn craft_paths(paths: &config::PathsConfig) -> novel_memory::Paths {
    novel_memory::Paths {
        cards: paths.cards.clone(),
        drafts: paths.drafts.clone(),
        arc_plan: paths.arc_plan.clone(),
        story_plan: paths.story_plan.clone(),
        chapter_plan: paths.chapter_plan.clone(),
        research: paths.research.clone(),
    }
}

/// 规则文件的 `memory` 节 → craft 的记忆配置。
fn craft_config(memory: &config::MemoryConfig) -> novel_memory::Config {
    novel_memory::Config {
        embedding_model: memory.embedding_model.clone(),
        created_at_base: memory.created_at_base,
        story_fact_sections: memory.story_fact_sections.clone(),
        card_related_field: memory.card_related_field.clone(),
        card_relation_section: memory.card_relation_section.clone(),
    }
}
