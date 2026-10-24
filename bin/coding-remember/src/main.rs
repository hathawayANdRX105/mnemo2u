//! The `coding-remember` entry point: parse and dispatch.
//!
//! The command's meaning lives in [`coding_remember::cli`]; this file only turns
//! a parsed command into a process. It is a separate target from the library so
//! the tests can exercise the CLI's contract without a `main` in the way.

use anyhow::{Context, Result};
use clap::Parser;
use coding_remember::cli::{run, Cli};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build the async runtime")?;
    runtime.block_on(run(cli))
}
