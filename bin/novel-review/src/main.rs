//! The novel-review binary: parse and dispatch.
//!
//! Everything lives in [`novel_review::cli`]; this file only turns a parsed
//! command into a process.

use anyhow::Result;
use clap::Parser;
use novel_review::cli::{run, Cli};

#[tokio::main]
async fn main() -> Result<()> {
    run(Cli::parse()).await
}
