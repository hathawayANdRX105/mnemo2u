//! The review craft: audit, statistics and consistency reports for a novel.
//!
//! The tool reads a novel's concept cards, outlines and drafts against a
//! rules file, and emits audit reports, mirror statistics trees and a
//! SQLite/FTS5 consistency index. It is the review half of the novel craft;
//! the memory half (`novel-memory`) sits next to it in `craft/`, and the
//! CLI's `memory` subcommand is where the two meet — the rules file is also
//! where the corpus layout is configured, so the same file that drives the
//! review can index the corpus.

pub mod audit;
pub mod cli;
pub mod config;
pub mod consistency;
pub mod input;
pub mod reports;
pub mod rules;
pub mod stats;
pub mod study;
pub mod style;
pub mod text;
pub mod tools;
