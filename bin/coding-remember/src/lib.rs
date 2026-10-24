//! The `coding-remember` command's meaning: the flags, the JSON a session
//! arrives as, and the scope a recall asks about.
//!
//! A separate library target from the binary, so the tests can pin the contract
//! a harness depends on — the JSON's field names, the empty-session spelling —
//! without running the binary. The keep-it-thin rule: this crate holds no
//! memory logic of its own, only the seam's parsing and shaping. Everything
//! about what a fact *is* lives in the craft.

pub mod cli;
