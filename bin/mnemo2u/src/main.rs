//! `mnemo2u` — the operational CLI (roadmap §4: `insert` / `query` / `admin`).
//!
//! The library is a set of traits and stores; this binary is the missing
//! surface that points them at a workspace and a model. Every subcommand is
//! one library call plus reporting — no logic of its own.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use mnemo2u::assemble::Assembled;
use mnemo2u::config::Config;
use mnemo2u::pipeline::InsertOutcome;

#[derive(Parser)]
#[command(
    name = "mnemo2u",
    version,
    about = "Memory/RAG library for coding harnesses"
)]
struct Cli {
    /// Config file (TOML). Defaults to `mnemo2u.toml` in the current directory.
    #[arg(short, long, default_value = "mnemo2u.toml", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Insert documents into the knowledge graph (files given as arguments).
    Insert {
        /// Document files. `-` reads stdin.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Query the graph and print an answer.
    Query {
        /// The question.
        query: String,
        /// Print the retrieved context only, without calling the model.
        #[arg(long)]
        context_only: bool,
    },
    /// Administrative operations.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand)]
enum AdminCommand {
    /// Replay queued derived-store writes (the repair queue).
    Flush,
    /// Rebuild the chunk vector index from the truth layer.
    Rebuild,
    /// Show store contents (documents, chunks, graph nodes/edges).
    Status,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match run(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: &Cli) -> Result<(), String> {
    let config = Config::load(&cli.config)?;
    match &cli.command {
        Command::Insert { files } => insert(&config, files).await,
        Command::Query {
            query,
            context_only,
        } => run_query(&config, query, *context_only).await,
        Command::Admin { command } => admin(&config, command).await,
    }
}

async fn insert(config: &Config, files: &[String]) -> Result<(), String> {
    let mut documents = Vec::with_capacity(files.len());
    for file in files {
        let content = if file == "-" {
            use std::io::Read;
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|error| format!("reading stdin: {error}"))?;
            buffer
        } else {
            std::fs::read_to_string(file).map_err(|error| format!("reading {file}: {error}"))?
        };
        documents.push(content);
    }

    // One assembly per run: the pipeline owns the stores, so a CLI call never
    // holds a second connection to the same files.
    let Assembled { pipeline, .. } = config.assemble().await?;
    let outcome = pipeline
        .insert(documents)
        .await
        .map_err(|error| format!("insert: {error}"))?;
    match outcome {
        InsertOutcome::AllDocsKnown => println!("all documents already known"),
        InsertOutcome::AllChunksKnown => println!("all chunks already known"),
        InsertOutcome::NoEntities => println!("no entities extracted"),
        InsertOutcome::Inserted {
            docs,
            chunks,
            entities,
            relations,
        } => {
            println!("inserted {docs} document(s): {chunks} chunks, {entities} entities, {relations} relations")
        }
    }
    pipeline
        .flush()
        .await
        .map_err(|error| format!("flush: {error}"))?;
    Ok(())
}

async fn run_query(config: &Config, text: &str, context_only: bool) -> Result<(), String> {
    let Assembled { pipeline, .. } = config.assemble().await?;
    let mut param = config.query.param()?;
    param.only_need_context = context_only;
    let answer = pipeline
        .query(text, &param)
        .await
        .map_err(|error| format!("query: {error}"))?;
    println!("{answer}");
    Ok(())
}

async fn admin(config: &Config, command: &AdminCommand) -> Result<(), String> {
    let Assembled { pipeline, .. } = config.assemble().await?;
    match command {
        AdminCommand::Flush => {
            pipeline
                .flush()
                .await
                .map_err(|error| format!("flush: {error}"))?;
            println!("flushed");
        }
        AdminCommand::Rebuild => {
            let rows = pipeline
                .rebuild_chunk_vectors()
                .await
                .map_err(|error| format!("rebuild: {error}"))?;
            println!("rebuilt {rows} chunk vector row(s)");
        }
        AdminCommand::Status => {
            let docs = pipeline
                .full_docs
                .all_keys()
                .await
                .map(|keys| keys.len().to_string())
                .map_err(|error| error.to_string());
            let chunks = pipeline
                .text_chunks
                .all_keys()
                .await
                .map(|keys| keys.len().to_string())
                .map_err(|error| error.to_string());
            let graph = pipeline
                .graph
                .snapshot()
                .await
                .map(|snapshot| {
                    format!(
                        "{} nodes, {} edges",
                        snapshot.nodes.len(),
                        snapshot.edges.len()
                    )
                })
                .map_err(|error| error.to_string());
            let repairs = pipeline
                .repair
                .pending()
                .await
                .map(|items| items.len().to_string())
                .map_err(|error| error.to_string());
            println!("documents:   {}", report(docs));
            println!("chunks:      {}", report(chunks));
            println!("graph:       {}", report(graph));
            println!("repairqueue: {}", report(repairs));
        }
    }
    Ok(())
}

fn report(result: Result<String, String>) -> String {
    match result {
        Ok(value) => value,
        Err(error) => format!("unavailable ({error})"),
    }
}
