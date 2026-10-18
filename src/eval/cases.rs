//! The self-built case sets (stage doc 06 T4): focus switching and
//! identifier recall.
//!
//! Both sets are deterministic by construction — the content turns are
//! literals, the distractor corpus is a pure function of `scale`, and the
//! embedding model is the deterministic mock — so a run is reproducible and
//! the snapshot id is stable across machines.

use super::{snapshot_id, snapshot_id_with_edges, Case, CaseEdge, CaseQuery, Conditions, Turn};

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
        // A near-miss for the query below: it shares `rotation`, `policy`
        // and `vault` but not `secret`, `manager`, `ninety` or `days`, so
        // AND-of-tokens correctly excludes it. A regression to OR matching
        // would pull it into the top-k and the `arrow version pin` marker
        // would land in a candidate — which is what makes the forbidden
        // assertion able to fail rather than decorative.
        Turn {
            turn_id: "b3".into(),
            role: "assistant".into(),
            content: "the arrow version pin was settled while drafting the rotation policy for the vault".into(),
            timestamp: 1_103,
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
    let conditions = Conditions {
        top_k: 5,
        min_recall: 1.0,
        forbid_wrong_evidence: true,
        expect_top_hit: false,
        target_recall: 1.0,
    };
    Case {
        case_id: "focus-switch-aba".into(),
        set: "focus-switch".into(),
        snapshot_id: snapshot_id("focus-switch-aba", &turns, &queries, scale, &conditions),
        scope,
        conversation_id: "conv-focus".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 3,
        conditions,
        edges: Vec::new(),
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
        // A near-miss for the multi-word query below: shares `chunk` and
        // `vector` but not `rebuild`, `entry` or `point`, so AND-of-tokens
        // excludes it and only OR matching would surface it.
        Turn {
            turn_id: "i6".into(),
            role: "user".into(),
            content: "the ERR-4041 failure surfaced while replaying the repair queue for the chunk vector write".into(),
            timestamp: 2_006,
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
        CaseQuery {
            // Multi-word on purpose. A single-phrase query (`src/pipeline.rs`)
            // cannot have a load-bearing forbidden marker: under AND-of-tokens
            // a fact either contains the whole phrase or none of it, so there
            // is no strict subset to exclude, and AND and OR behave
            // identically for one token. With several tokens a near-miss fact
            // can share some of them, which is what lets this negative fail
            // if the matching semantics regress.
            text: "chunk vector rebuild entry point".into(),
            expected: vec!["src/pipeline.rs".into()],
            forbidden: vec!["ERR-4041".into()],
        },
    ];
    let scope = super::super::ScopeKey {
        workspace_id: "eval-ws".into(),
        session_id: None,
        task_id: None,
    };
    let conditions = Conditions {
        top_k: 3,
        min_recall: 1.0,
        forbid_wrong_evidence: true,
        // This set's claim is a ranking claim — an exact identifier must
        // come back first, not merely somewhere in the top-k.
        expect_top_hit: true,
        target_recall: 1.0,
    };
    Case {
        case_id: "identifier-exact".into(),
        set: "identifier".into(),
        snapshot_id: snapshot_id("identifier-exact", &turns, &queries, scale, &conditions),
        scope,
        conversation_id: "conv-ident".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 2,
        conditions,
        edges: Vec::new(),
    }
}

/// Every self-built case at a given scale.
pub fn all(scale: usize) -> Vec<Case> {
    vec![focus_switch_case(scale), identifier_case(scale)]
}

/// The headroom case at a given scale: the answer is three hops out.
///
/// The corpus is a chain — a seed fact the query lexically matches, then two
/// more facts, then the one that carries the marker. The graph arm seeds from
/// lexical and vector hits and expands, so at the default two hops it walks
/// the seed and its two neighbours and stops; the fact holding the marker
/// sits one hop beyond that and never enters fusion. Three hops reaches it.
///
/// This is the direction the search can actually take. Every other knob is
/// inert here: the corpus has no corrections to weight, no judge runs so no
/// relation is inferred, and the candidate cap is above the harness's own
/// per-arm limit. `summary_chars` cannot help either — the marker sits well
/// inside the cut, so a longer summary adds nothing new to compare against.
pub fn graph_hop_case(scale: usize) -> Case {
    let turns = vec![
        Turn {
            turn_id: "g0".into(),
            role: "user".into(),
            content: "the relief valve for the northern pump was scheduled for inspection".into(),
            timestamp: 5_000,
        },
        Turn {
            turn_id: "g1".into(),
            role: "assistant".into(),
            content: "the inspection crew routes through the east gate before the valve yard"
                .into(),
            timestamp: 5_001,
        },
        Turn {
            turn_id: "g2".into(),
            role: "assistant".into(),
            content: "the yard log records the pass number used by the crew that day".into(),
            timestamp: 5_002,
        },
        Turn {
            turn_id: "g3".into(),
            role: "assistant".into(),
            content: "the pass number stamped for the relief valve inspection was VLV-2210".into(),
            timestamp: 5_003,
        },
    ];
    let queries = vec![CaseQuery {
        // Every term co-occurs in the seed turn, so the lexical arm matches
        // exactly that fact and the walk starts where the chain starts.
        text: "relief valve northern pump inspection".into(),
        // Only the last fact carries it: at two hops that fact is never
        // reached, and at three it is.
        expected: vec!["VLV-2210".into()],
        forbidden: vec![],
    }];
    let scope = super::super::ScopeKey {
        workspace_id: "eval-ws".into(),
        session_id: None,
        task_id: None,
    };
    // A chain, one hop per link. `Related` carries a multiplier of 1.0, so the
    // weight is what the graph arm adds up.
    let edges = vec![
        CaseEdge {
            src: 0,
            dst: 1,
            rel: crate::core::types::RelKind::Related,
            weight: 1.0,
        },
        CaseEdge {
            src: 1,
            dst: 2,
            rel: crate::core::types::RelKind::Related,
            weight: 1.0,
        },
        CaseEdge {
            src: 2,
            dst: 3,
            rel: crate::core::types::RelKind::Related,
            weight: 1.0,
        },
    ];
    let conditions = Conditions {
        top_k: 5,
        // No floor of its own: the quality floor is not what keeps this case
        // honest, the missing marker is.
        min_recall: 0.0,
        forbid_wrong_evidence: true,
        expect_top_hit: false,
        // The ceiling: this case should be answered.
        target_recall: 1.0,
    };
    Case {
        case_id: "graph-hop".into(),
        set: "graph-hop".into(),
        snapshot_id: snapshot_id_with_edges(
            "graph-hop",
            &turns,
            &queries,
            scale,
            &conditions,
            &edges,
        ),
        scope,
        conversation_id: "conv-hop".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 1,
        conditions,
        edges,
    }
}

/// The headroom set: two cases, two queries, so the noise floor is a half.
///
/// Two is the smallest useful number. One query makes the floor equal the
/// whole set, and a gain that fixes that one query is exactly what the floor
/// exists to refuse — one query's worth of movement. Two queries let a fix
/// that holds for both clear it.
pub fn summary_cut_all(scale: usize) -> Vec<Case> {
    vec![graph_hop_case(scale), graph_hop_case(scale)]
}

/// A holdout case that no searchable knob can move.
///
/// Short turns, every marker inside the default cut, no edges at all, and a
/// fresh vocabulary unrelated to any other set — so the case measures
/// generalization rather than a shared word.
///
/// All the graph knobs act on edges that do not exist here, so they cannot
/// change its numbers at all. `summary_chars` cannot either: every marker
/// already sits inside the cut, so a longer summary adds nothing new to
/// compare. What it buys is a set that only ever catches a regression the
/// search did not predict.
///
/// Four queries, so that with the two external slices the holdout reaches six:
/// a one-query holdout can swing on a single flip, and a set that coarse cannot
/// tell a regression from noise. Each query's terms live in its marker-bearing
/// turn and nowhere else — the lexical arm is an AND-of-tokens, so a term that
/// also matched a neighbouring turn would rank that one and let the markers
/// fall out of the top-k.
pub fn held_out_case(scale: usize) -> Case {
    let turns = vec![
        Turn {
            turn_id: "h0".into(),
            role: "assistant".into(),
            content: "the rye starter was refreshed at dawn and the dough rested until noon".into(),
            timestamp: 4_000,
        },
        Turn {
            turn_id: "h1".into(),
            role: "user".into(),
            content: "which shelf holds the linen towels and who folded them".into(),
            timestamp: 4_001,
        },
        Turn {
            turn_id: "h2".into(),
            role: "assistant".into(),
            content: "the second shelf holds the folded linen towels and mara folded them".into(),
            timestamp: 4_002,
        },
        Turn {
            turn_id: "h3".into(),
            role: "assistant".into(),
            content: "the copper kettle was scoured with lemon juice before the winter storage"
                .into(),
            timestamp: 4_003,
        },
        Turn {
            turn_id: "h4".into(),
            role: "assistant".into(),
            content: "the attic hatch sticks after the rain because the frame swelled unevenly"
                .into(),
            timestamp: 4_004,
        },
    ];
    let queries = vec![
        CaseQuery {
            text: "rye starter refreshed dough rested".into(),
            expected: vec!["rye starter".into()],
            forbidden: vec!["VLV-2210".into()],
        },
        CaseQuery {
            // Every term co-occurs in the marker-bearing turn and nowhere
            // else, so the lexical arm AND-matches exactly the fact that holds
            // the answer. A query that also matched a neighbouring turn would
            // rank that one and let the markers fall out of the top-k.
            text: "linen towels second shelf mara folded".into(),
            expected: vec!["mara folded".into(), "second shelf".into()],
            forbidden: vec!["relief valve".into()],
        },
        CaseQuery {
            // Same rule as the rest: the terms live in `h3` and nowhere else.
            text: "copper kettle scoured lemon juice winter storage".into(),
            expected: vec!["copper kettle".into()],
            forbidden: vec!["linen towels".into()],
        },
        CaseQuery {
            // And here: `h4` only.
            text: "attic hatch sticks frame swelled".into(),
            expected: vec!["frame swelled".into()],
            forbidden: vec!["copper kettle".into()],
        },
    ];
    let scope = super::super::ScopeKey {
        workspace_id: "eval-ws".into(),
        session_id: None,
        task_id: None,
    };
    let conditions = Conditions {
        top_k: 5,
        min_recall: 1.0,
        forbid_wrong_evidence: true,
        expect_top_hit: false,
        target_recall: 1.0,
    };
    Case {
        case_id: "held-out".into(),
        set: "held-out".into(),
        snapshot_id: snapshot_id_with_edges("held-out", &turns, &queries, scale, &conditions, &[]),
        scope,
        conversation_id: "conv-held".into(),
        turns,
        distractor_sessions: scale,
        queries,
        repetitions: 1,
        conditions,
        edges: Vec::new(),
    }
}

/// The holdout set, in the shape the search gets it: this new case plus the
/// external slices, so the holdout keeps what it had and grows to a query
/// count where one query cannot swing the whole set.
pub fn held_out_all(scale: usize) -> Vec<Case> {
    vec![held_out_case(scale)]
}
