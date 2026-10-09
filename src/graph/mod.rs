//! Graph pipeline (nano-graphrag core, R1):
//! chunking -> entity/relation extraction -> merge/dedup -> graph build ->
//! community detection (leiden-rs) -> community reports.
//!
//! Ported from nano-graphrag `_op.py`; storage goes to the three-store layer,
//! not the reference's json/nano-vectordb/networkx.
//!
//! NOT YET IMPLEMENTED: R1.
