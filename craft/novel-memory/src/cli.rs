//! The `memory` subcommands: write the three layers in, and read them back.
//!
//! Four subcommands:
//!
//! - `index`: scan a novel directory → the three layers → write them into mnemo2u
//! - `query`: semantic search
//! - `relations`: the relations of one entity
//! - `timeline`: the fact stream in story order
//!
//! The three stores land under the novel directory, not next to the tool:
//! changing tools must not lose memory.
//!
//! The timeline is story time, not writing order. See [`crate::storytime`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;
use mnemo2u::core::types::ScopeKey;
use serde_json::json;

use crate::config::{Config, Paths};
use crate::corpus::{self, Corpus};
use crate::query::{self, Hit};
use crate::store::{Indexer, MemoryBackend};
use crate::storytime::Layer;

/// The `memory` subcommands.
///
/// The caller supplies the craft's own configuration; this crate never reads a
/// rules file, so it can be driven from any binary.
#[derive(Subcommand)]
pub enum MemoryCmd {
    /// Scan a novel directory and write the setting / fact / relation layers.
    Index {
        /// The novel directory (e.g. `novel1`).
        novel_dir: PathBuf,
        /// Report what would be written, without touching the stores.
        #[arg(long)]
        dry_run: bool,
    },
    /// Semantic search.
    Query {
        /// The novel directory.
        novel_dir: PathBuf,
        /// The question.
        question: String,
        /// How many hits (default 10).
        #[arg(long, default_value_t = 10)]
        top: usize,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// The relations of one entity.
    Relations {
        /// The novel directory.
        novel_dir: PathBuf,
        /// The entity name (a card title or alias).
        entity: String,
    },
    /// The fact stream in story order.
    Timeline {
        /// The novel directory.
        novel_dir: PathBuf,
        /// Restrict to one story (a story number).
        #[arg(long, value_name = "N")]
        story: Option<String>,
    },
}

/// Run a `memory` subcommand, returning the process exit code.
pub async fn run(cmd: &MemoryCmd, paths: &Paths, config: &Config) -> Result<i32> {
    match cmd {
        MemoryCmd::Index {
            novel_dir, dry_run, ..
        } => index(novel_dir, *dry_run, paths, config).await,
        MemoryCmd::Query {
            novel_dir,
            question,
            top,
            json: as_json,
            ..
        } => {
            let backend = open_backend(novel_dir, paths, config).await?;
            let hits = query::query(&backend, &scope_of(novel_dir), question, *top).await?;
            print_hits(&hits, *as_json);
            Ok(0)
        }
        MemoryCmd::Relations {
            novel_dir, entity, ..
        } => {
            let backend = open_backend(novel_dir, paths, config).await?;
            let hits = query::relations(&backend, &scope_of(novel_dir), entity).await?;
            if hits.is_empty() {
                println!("no relations found for `{entity}`");
            } else {
                for hit in &hits {
                    println!("{}", hit.line());
                }
            }
            Ok(0)
        }
        MemoryCmd::Timeline {
            novel_dir, story, ..
        } => {
            let backend = open_backend(novel_dir, paths, config).await?;
            let hits = query::timeline(&backend, &scope_of(novel_dir), story.as_deref()).await?;
            if hits.is_empty() {
                println!("no facts; run the index subcommand first");
            } else {
                for hit in &hits {
                    println!("{}", hit.line());
                }
            }
            Ok(0)
        }
    }
}

/// The novel directory → the root of the three memory stores.
fn memo_root(novel_dir: &Path, paths: &Paths) -> PathBuf {
    novel_dir.join(&paths.memo_root)
}

/// The novel directory → the scope.
///
/// `workspace_id` is the novel directory name (one workspace per book);
/// `session_id` stays empty (a book's setting is not per-session); `task_id`
/// stays empty for cross-story queries.
fn scope_of(novel_dir: &Path) -> ScopeKey {
    ScopeKey {
        workspace_id: novel_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "novel".to_string()),
        session_id: None,
        task_id: None,
    }
}

/// Open the three stores.
async fn open_backend(novel_dir: &Path, paths: &Paths, config: &Config) -> Result<MemoryBackend> {
    let root = memo_root(novel_dir, paths);
    MemoryBackend::open(&root, config)
        .await
        .with_context(|| format!("failed to open the memory stores: {}", root.display()))
}

/// Index one novel directory.
async fn index(novel_dir: &Path, dry_run: bool, paths: &Paths, config: &Config) -> Result<i32> {
    let scanned: Corpus = corpus::scan(novel_dir, paths, config);

    if dry_run {
        let mut by_layer: [usize; 3] = [0, 0, 0];
        for slot in &scanned.slots {
            by_layer[layer_index(slot.layer)] += 1;
        }
        println!("setting  {}", by_layer[0]);
        println!("fact     {}", by_layer[1]);
        println!("relation {}", by_layer[2]);
        println!("edges    {}", scanned.edges.len());
        println!("units    {}", scanned.units.len());
        println!("(dry-run: nothing written)");
        return Ok(0);
    }

    let backend = open_backend(novel_dir, paths, config).await?;
    let indexer = Indexer::new(&backend, scope_of(novel_dir), config.clone());
    let report = indexer.index(&scanned).await?;

    println!("written    {}", report.written);
    println!("unchanged  {}", report.unchanged);
    println!("superseded {}", report.superseded);
    println!("edges      {}", report.edges);
    for (layer, count) in &report.by_layer {
        println!("{layer} {count}");
    }
    Ok(0)
}

/// Print hits.
fn print_hits(hits: &[Hit], as_json: bool) {
    if as_json {
        let payload = json!({
            "hits": hits
                .iter()
                .map(|hit| json!({
                    "id": hit.id,
                    "text": hit.text,
                    "score": hit.score,
                    "layer": hit.layer.label(),
                    "story": hit.story,
                    "source": hit.source,
                }))
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return;
    }
    if hits.is_empty() {
        println!("no hits");
        return;
    }
    for hit in hits {
        println!("{}", hit.line());
    }
}

/// Layer → the counting index.
fn layer_index(layer: Layer) -> usize {
    match layer {
        Layer::Setting => 0,
        Layer::Fact => 1,
        Layer::Relation => 2,
    }
}
