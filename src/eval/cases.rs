//! The self-built case sets (stage doc 06 T4): focus switching and
//! identifier recall.
//!
//! Both sets are deterministic by construction — the content turns are
//! literals, the distractor corpus is a pure function of `scale`, and the
//! embedding model is the deterministic mock — so a run is reproducible and
//! the snapshot id is stable across machines.

use super::{snapshot_id, Case, CaseQuery, Conditions, Turn};

/// A→B→A: three sessions about topic A, three about topic B, then a return
/// to A asking about the early decision. The recall query must still surface
/// the early-A evidence, and must not surface B-only evidence as an answer.
pub fn focus_switch_case(scale: usize) -> Case {
    let turns = vec![
        // --- Session A: the deploy key rotation policy. ---
        Turn {
            turn_id: "a0".into(),
            role: "user".into(),
            content: "we need to decide how the deploy key rotates and where the value lives".into(),
            timestamp: 1_000,
        },
        Turn {
            turn_id: "a1".into(),
            role: "assistant".into(),
            content: "the rotation policy is ninety days and the rotated value is stored in the vault secret manager".into(),
            timestamp: 1_001,
        },
        Turn {
            turn_id: "a2".into(),
            role: "user".into(),
            content: "confirm the vault path and the owner of the rotation schedule".into(),
            timestamp: 1_002,
        },
        // --- Session B: an unrelated schema migration. ---
        Turn {
            turn_id: "b0".into(),
            role: "user".into(),
            content: "moving on to the lancedb schema migration and the arrow version pin".into(),
            timestamp: 1_100,
        },
        Turn {
            turn_id: "b1".into(),
            role: "assistant".into(),
            content: "the arrow version pin stays at 58 and the migration drops the column before re-adding it".into(),
            timestamp: 1_101,
        },
        Turn {
            turn_id: "b2".into(),
            role: "user".into(),
            content: "the migration needs a rollback plan for the pinned arrow version".into(),
            timestamp: 1_102,
        },
        // --- Return to A: ask about the early decision again. ---
        Turn {
            turn_id: "a3".into(),
            role: "user".into(),
            content: "back to the deploy key policy what window did we settle on and where is the value kept".into(),
            timestamp: 1_200,
        },
        Turn {
            turn_id: "a4".into(),
            role: "assistant".into(),
            content: "ninety days and the vault secret manager as decided earlier".into(),
            timestamp: 1_201,
        },
    ];
    let queries = vec![CaseQuery {
        // Term-dense rather than prose, and every term co-occurs in the
        // early-A fact: the lexical arm AND-matches its terms, so a query
        // built from words spread across several facts matches nothing.
        text: "rotation policy vault secret manager ninety days".into(),
        expected: vec!["ninety days".into(), "vault secret manager".into()],
        // B-session vocabulary: an answer built from it is the wrong evidence.
        forbidden: vec!["arrow version pin".into(), "schema migration".into()],
    }];
    let scope = super::super::ScopeKey {
        workspace_id: "eval-ws".into(),
        session_id: None,
        task_id: None,
    };
    Case {
        case_id: "focus-switch-aba".into(),
        set: "focus-switch".into(),
        snapshot_id: snapshot_id("focus-switch-aba", &turns, &queries, scale),
        scope,
        conversation_id: "conv-focus".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 3,
        conditions: Conditions {
            top_k: 5,
            min_recall: 1.0,
            forbid_wrong_evidence: true,
        },
    }
}

/// Exact-identifier recall: paths, error codes and symbol names. The claim
/// under test is the one R3.AC1 makes — a lexical arm beats a pure vector
/// arm on exact identifiers — so the query *is* the identifier and the
/// expected marker is the fact that contains it.
pub fn identifier_case(scale: usize) -> Case {
    let turns = vec![
        Turn {
            turn_id: "i0".into(),
            role: "assistant".into(),
            content: "the chunk vector rebuild entry point lives in src/pipeline.rs and reads every row back from the truth layer".into(),
            timestamp: 2_000,
        },
        Turn {
            turn_id: "i1".into(),
            role: "user".into(),
            content: "the build failed with ERR-4041 while the repair queue replayed the failed derived write".into(),
            timestamp: 2_001,
        },
        Turn {
            turn_id: "i2".into(),
            role: "assistant".into(),
            content: "rrf_fuse is the deterministic rank fusion both the recall path and the gate rely on".into(),
            timestamp: 2_002,
        },
        Turn {
            turn_id: "i3".into(),
            role: "user".into(),
            content: "the workspace config is read from /etc/mnemo2u/config.toml before any store opens".into(),
            timestamp: 2_003,
        },
        Turn {
            turn_id: "i4".into(),
            role: "assistant".into(),
            content: "ScopeKey::admits is the shared pre-filter predicate every recall arm applies inside its own query".into(),
            timestamp: 2_004,
        },
        Turn {
            turn_id: "i5".into(),
            role: "user".into(),
            content: "the turso facts table keys rows by workspace session and task before the fact id".into(),
            timestamp: 2_005,
        },
    ];
    let queries = vec![
        CaseQuery {
            text: "src/pipeline.rs".into(),
            expected: vec!["src/pipeline.rs".into()],
            forbidden: vec!["ERR-4041".into(), "rrf_fuse".into()],
        },
        CaseQuery {
            text: "ERR-4041".into(),
            expected: vec!["ERR-4041".into()],
            forbidden: vec!["src/pipeline.rs".into()],
        },
        CaseQuery {
            text: "rrf_fuse".into(),
            expected: vec!["rrf_fuse".into()],
            forbidden: vec!["config.toml".into()],
        },
        CaseQuery {
            text: "/etc/mnemo2u/config.toml".into(),
            expected: vec!["config.toml".into()],
            forbidden: vec![],
        },
    ];
    let scope = super::super::ScopeKey {
        workspace_id: "eval-ws".into(),
        session_id: None,
        task_id: None,
    };
    Case {
        case_id: "identifier-exact".into(),
        set: "identifier".into(),
        snapshot_id: snapshot_id("identifier-exact", &turns, &queries, scale),
        scope,
        conversation_id: "conv-ident".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 2,
        conditions: Conditions {
            top_k: 3,
            min_recall: 1.0,
            forbid_wrong_evidence: true,
        },
    }
}

/// Every self-built case at a given scale.
pub fn all(scale: usize) -> Vec<Case> {
    vec![focus_switch_case(scale), identifier_case(scale)]
}
