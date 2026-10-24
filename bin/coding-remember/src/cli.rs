//! `coding-remember`: write and recall a coding session's memory.
//!
//! This binary exists for *other* repos. A coding harness (ferrite) wants what
//! this repo built — episodes, rules and procedures with scopes that survive
//! across sessions — and it should not have to depend on this workspace to get
//! it. So the seam is a process: the harness shells out, this reads its store
//! argument and prints facts on stdout.
//!
//! The command it runs is small on purpose. `remember` takes a session on stdin
//! (one JSON object), `recall` prints what a scope can see, and `edges` prints
//! the derivation graph. Everything heavy stays in the craft's library, where
//! the tests are.
//!
//! ## Exit codes
//!
//! `0` success, `1` an operational failure (a store that will not open, a
//! session that will not parse). A *query that matched nothing* is still `0` —
//! an empty result is an answer, not an error — so a caller can distinguish
//! "the store is broken" from "nothing was remembered".

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use coding_memory::store::{Reader, SessionRecord, WriteReport};
use coding_memory::ScopeKey;
use coding_memory::{Coder, Config, MemoryBackend, TaskScope};

/// Write and recall a coding session's memory.
#[derive(Debug, Parser)]
#[command(
    name = "coding-remember",
    version,
    about = "Write and recall a coding session's memory"
)]
pub struct Cli {
    /// The store directory (created if absent).
    #[arg(long, short)]
    pub store: PathBuf,
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// What to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Write one session's episodes, rules and procedures (JSON on stdin).
    Remember {
        /// The repository the session belongs to (the workspace).
        #[arg(long)]
        project: String,
        /// The execution session.
        #[arg(long)]
        session: String,
        /// The task family, e.g. `topic:release`. Absent means the whole project.
        #[arg(long)]
        topic: Option<String>,
    },
    /// Print the facts a scope can see, as JSON.
    Recall {
        /// The repository (the workspace).
        #[arg(long)]
        project: String,
        /// The session to recall from. Absent means every session in the project.
        #[arg(long)]
        session: Option<String>,
        /// The task family to recall from. Absent means every topic.
        #[arg(long)]
        topic: Option<String>,
        /// What to print: `rules`, `procedures`, `episodes`, or `all`.
        #[arg(long, default_value = "all")]
        kind: String,
    },
    /// Print the derivation edges under a scope: which episode a rule came
    /// from, and which rule a procedure came from.
    Edges {
        /// The repository (the workspace).
        #[arg(long)]
        project: String,
    },
}

/// Open the stores for a command that writes, or for one that reads.
async fn open(store: &std::path::Path) -> Result<MemoryBackend> {
    MemoryBackend::open(store, &Config::default())
        .await
        .with_context(|| format!("open the store at {}", store.display()))
}

/// The scope a recall or a write is asking about.
///
/// An absent session *and* an empty one both mean "no session constraint", so
/// they map to the same `session_id`. Leaving this to the caller would make
/// `--session ""` a scope that matches nothing, which is a confusing way to
/// spell "everything" — and it is the difference between a project-wide recall
/// and an empty answer.
pub fn scope_of_from_flags(
    project: &str,
    session: Option<&str>,
    topic: Option<&str>,
) -> Result<ScopeKey> {
    let session = session.filter(|value| !value.is_empty());
    let topic = topic.filter(|value| !value.is_empty());
    Ok(ScopeKey {
        workspace_id: project.to_string(),
        session_id: session.map(str::to_string),
        task_id: topic.map(str::to_string),
    })
}

/// Read the whole command.
pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Remember {
            project,
            session,
            topic,
        } => {
            // A session arrives on stdin as one JSON object. Reading it from
            // there rather than a file keeps the harness's side of the contract
            // to `echo … | coding-remember remember`, with nothing to clean up.
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .context("read the session from stdin")?;
            let record: SessionRecord =
                serde_json::from_str(&text).context("parse the session as JSON")?;
            let task_scope = TaskScope {
                project: project.clone(),
                session: session.clone(),
                topic: topic.clone(),
            };
            let backend = open(&cli.store).await?;
            let coder = Coder::new(&backend, Config::default());
            let report: WriteReport = coder
                .record(&record, &task_scope)
                .await
                .context("write the session")?;
            print!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "store": cli.store.display().to_string(),
                    "project": project,
                    "session": session,
                    "topic": topic,
                    "written": report.written,
                    "unchanged": report.unchanged,
                    "superseded": report.superseded,
                    "edges": report.edges,
                }))
                .context("render the write report")?
            );
            println!();
        }
        Command::Recall {
            project,
            session,
            topic,
            kind,
        } => {
            let backend = open(&cli.store).await?;
            let reader = Reader::new(&backend);
            let scope = scope_of_from_flags(&project, session.as_deref(), topic.as_deref())?;

            let mut out = serde_json::Map::new();
            let mut push = |label: &str, facts: Vec<coding_memory::Fact>| {
                out.insert(
                    label.to_string(),
                    serde_json::to_value(facts).unwrap_or_default(),
                );
            };
            let wants_all = kind == "all";
            if wants_all || kind == "rules" {
                push(
                    "rules",
                    reader.rules(&scope).await.context("read the rules")?,
                );
            }
            if wants_all || kind == "procedures" {
                push(
                    "procedures",
                    reader
                        .procedures(&scope)
                        .await
                        .context("read the procedures")?,
                );
            }
            if wants_all || kind == "episodes" {
                push(
                    "episodes",
                    reader.episodes(&scope).await.context("read the episodes")?,
                );
            }
            if !wants_all && !["rules", "procedures", "episodes"].contains(&kind.as_str()) {
                anyhow::bail!("unknown kind {kind}: expected rules, procedures, episodes or all");
            }
            print!(
                "{}",
                serde_json::to_string_pretty(&serde_json::Value::Object(out))
                    .context("render the recall")?
            );
            println!();
        }
        Command::Edges { project } => {
            let backend = open(&cli.store).await?;
            let reader = Reader::new(&backend);
            let scope = scope_of_from_flags(&project, None, None)?;
            let edges = reader
                .derivation_edges(&scope)
                .await
                .context("read the derivation edges")?;
            print!(
                "{}",
                serde_json::to_string_pretty(&edges).context("render the edges")?
            );
            println!();
        }
    }
    Ok(())
}
