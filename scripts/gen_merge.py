#!/usr/bin/env python3
"""Generate merge golden fixtures from the LightRAG algorithm.

The merge rules below are copied from `refs/LightRAG/lightrag/operate.py`
(`_merge_nodes_then_upsert` :2429, `_merge_edges_then_upsert` :2782,
`_combine_descriptions_dedup` :2384, `_handle_entity_relation_summary` :372,
`merge_source_ids` utils.py:7183, `apply_source_ids_limit` utils.py:7244),
with a dict-backed graph mock standing in for the storage layer and a
word-count proxy standing in for the tokenizer.

Notes on set ordering: the reference joins `source_id` from a Python `set`,
whose iteration order is implementation-defined, so the fixture records
`source_id` as a *sorted list* and the Rust test compares sets.

Notes on the tokenizer: `_handle_entity_relation_summary`'s tier decision is
token-based (tiktoken). The fixture keeps every case far from the boundaries
(short fragments for tier 1, >1500 words for tier 2), so a whitespace word
count — a lower bound on the token count — makes the same decision.

Usage:
    python3 scripts/gen_merge.py > tests/fixtures/merge_golden.json
"""

import copy
import json
import re
import sys
from collections import Counter

GRAPH_FIELD_SEP = "<SEP>"
FORCE_LLM_SUMMARY_ON_MERGE = 8
SUMMARY_MAX_TOKENS = 1200
SUMMARY_CONTEXT_SIZE = 12000
MAX_SOURCE_IDS_PER_ENTITY = 300
MAX_SOURCE_IDS_PER_RELATION = 300
SOURCE_IDS_LIMIT_METHOD = "KEEP"
UNKNOWN_SOURCE = "unknown_source"


def sanitize_text_for_encoding(text, strip=True):
    if strip:
        text = text.strip()
    if not text:
        return text
    text = re.sub(r"[\x00-\x1f\x7f-\x9f]", "", text)
    return text.strip() if strip else text


def split_string_by_multi_markers(content, markers):
    if not markers:
        return [content]
    results = re.split("|".join(re.escape(marker) for marker in markers), content)
    return [r.strip() for r in results if r.strip()]


class DictGraph:
    def __init__(self, nodes=None, edges=None):
        # Deep copy: the caller's dicts are recorded verbatim as the case
        # `seed`, and `upsert_*` mutates inner dicts in place.
        self.nodes = copy.deepcopy(nodes or {})
        self.edges = copy.deepcopy(edges or {})

    def get_node(self, node_id):
        return self.nodes.get(node_id)

    def upsert_node(self, node_id, node_data):
        self.nodes.setdefault(node_id, {}).update(node_data)

    def has_node(self, node_id):
        return node_id in self.nodes

    def get_edge(self, src, tgt):
        return self.edges.get((src, tgt))

    def has_edge(self, src, tgt):
        return (src, tgt) in self.edges

    def upsert_edge(self, src, tgt, edge_data):
        self.edges.setdefault((src, tgt), {}).update(edge_data)


def handle_entity_relation_summary(name, description_list, llm_response, calls):
    """`_handle_entity_relation_summary` (operate.py:372) tier decision only.

    Tier 1 (no LLM): fewer than 8 fragments and under 1200 tokens -> join.
    Tier 2 (LLM): one summary call over the joined fragments.
    """
    joined = GRAPH_FIELD_SEP.join(description_list)
    tokens = len(re.findall(r"\S+", joined))
    if len(description_list) < FORCE_LLM_SUMMARY_ON_MERGE and tokens < SUMMARY_MAX_TOKENS:
        return joined, False
    calls.append({"name": name, "description_list": list(description_list)})
    return llm_response, True


def merge_source_ids(existing, incoming, limit=MAX_SOURCE_IDS_PER_ENTITY, method=SOURCE_IDS_LIMIT_METHOD):
    merged = []
    for source_id in list(existing) + list(incoming):
        if source_id and source_id not in merged:
            merged.append(source_id)
    if limit and len(merged) > limit:
        merged = merged[-limit:] if method == "FIFO" else merged[:limit]
    return merged


def apply_source_ids_limit(ids, limit, method):
    if not limit or len(ids) <= limit:
        return list(ids)
    return ids[-limit:] if method == "FIFO" else ids[:limit]


def combine_descriptions(already_description, new_descriptions):
    combined, seen = [], set()

    def add(descriptions):
        for desc in descriptions:
            sanitized = sanitize_text_for_encoding(desc)
            if sanitized and sanitized not in seen:
                seen.add(sanitized)
                combined.append(sanitized)

    add(already_description)
    add(new_descriptions)
    return combined


def merge_nodes_then_upsert(entity_name, nodes_data, graph, llm_response, calls,
                            file_path_limit=None):
    already_entity_types, already_source_ids, already_description, already_file_paths = [], [], [], []
    already_node = graph.get_node(entity_name)
    if already_node:
        existing_type = already_node.get("entity_type")
        if not isinstance(existing_type, str) or not existing_type.strip():
            existing_type = "UNKNOWN"
        if "," in existing_type:
            tokens = [t.strip() for t in existing_type.split(",")]
            non_empty = [t for t in tokens if t]
            existing_type = non_empty[0] if non_empty else "UNKNOWN"
        already_entity_types.append(existing_type)
        already_source_ids.extend((already_node.get("source_id") or "").split(GRAPH_FIELD_SEP))
        already_file_paths.extend((already_node.get("file_path") or UNKNOWN_SOURCE).split(GRAPH_FIELD_SEP))
        existing_desc = (already_node.get("description") or "").strip()
        if existing_desc:
            already_description.extend(existing_desc.split(GRAPH_FIELD_SEP))

    new_source_ids = [dp["source_id"] for dp in nodes_data if dp.get("source_id")]
    full_source_ids = merge_source_ids(
        [c for c in already_source_ids if c], new_source_ids, MAX_SOURCE_IDS_PER_ENTITY
    )
    source_ids = apply_source_ids_limit(full_source_ids, MAX_SOURCE_IDS_PER_ENTITY, SOURCE_IDS_LIMIT_METHOD)

    if SOURCE_IDS_LIMIT_METHOD == "KEEP":
        allowed = set(source_ids)
        nodes_data = [
            dp for dp in nodes_data
            if not dp.get("source_id")
            or dp["source_id"] in allowed
            or dp["source_id"] in full_source_ids
        ]

    # `max` over the counter keeps the reference's tie-break: Counter preserves
    # first-insertion order and `max` returns the first maximal element.
    entity_type = max(
        Counter([dp["entity_type"] for dp in nodes_data] + already_entity_types).items(),
        key=lambda item: item[1],
    )[0]

    unique_nodes = {}
    for dp in nodes_data:
        desc = dp.get("description")
        if desc and desc not in unique_nodes:
            unique_nodes[desc] = dp
    sorted_nodes = sorted(
        unique_nodes.values(), key=lambda x: (x.get("timestamp", 0), -len(x.get("description", "")))
    )
    description_list = combine_descriptions(
        already_description, [dp["description"] for dp in sorted_nodes]
    )
    if not description_list:
        description_list = [f"Entity {entity_name}"]

    description, _llm_used = handle_entity_relation_summary(
        entity_name, description_list, llm_response, calls
    )

    file_paths_list, seen_paths = [], set()
    for fp in already_file_paths:
        if fp and fp not in seen_paths:
            file_paths_list.append(fp)
            seen_paths.add(fp)
    for dp in nodes_data:
        fp = dp.get("file_path")
        if fp and fp not in seen_paths:
            file_paths_list.append(fp)
            seen_paths.add(fp)
    max_file_paths = file_path_limit if file_path_limit is not None else len(file_paths_list) + 1
    if len(file_paths_list) > max_file_paths:
        file_paths_list = file_paths_list[:max_file_paths]
        file_paths_list.append(f"...TRUNCATED...({SOURCE_IDS_LIMIT_METHOD})")
    file_path = GRAPH_FIELD_SEP.join(file_paths_list) if file_paths_list else UNKNOWN_SOURCE

    truncate = "KEEP Old" if len(source_ids) < len(full_source_ids) else ""
    node_data = {
        "entity_type": entity_type,
        "description": description,
        "source_id": GRAPH_FIELD_SEP.join(source_ids),
        "file_path": file_path,
        "created_at": 0,
        "truncate": truncate,
    }
    graph.upsert_node(entity_name, node_data=node_data)
    node_data = dict(node_data)
    node_data["entity_name"] = entity_name
    return node_data


def merge_edges_then_upsert(src_id, tgt_id, edges_data, graph, llm_response, calls,
                            file_path_limit=None):
    already_weights, already_source_ids, already_description = [], [], []
    already_keywords, already_file_paths = [], []
    already_edge = graph.get_edge(src_id, tgt_id) if graph.has_edge(src_id, tgt_id) else None
    if already_edge:
        already_weights.append(already_edge.get("weight", 1.0))
        if already_edge.get("source_id") is not None:
            already_source_ids.extend(already_edge["source_id"].split(GRAPH_FIELD_SEP))
        if already_edge.get("file_path") is not None:
            already_file_paths.extend(already_edge["file_path"].split(GRAPH_FIELD_SEP))
        if already_edge.get("description") is not None:
            already_description.extend(already_edge["description"].split(GRAPH_FIELD_SEP))
        if already_edge.get("keywords") is not None:
            already_keywords.extend(
                split_string_by_multi_markers(already_edge["keywords"], [GRAPH_FIELD_SEP])
            )

    new_source_ids = [dp["source_id"] for dp in edges_data if dp.get("source_id")]
    full_source_ids = merge_source_ids(
        [c for c in already_source_ids if c], new_source_ids, MAX_SOURCE_IDS_PER_RELATION
    )
    source_ids = apply_source_ids_limit(full_source_ids, MAX_SOURCE_IDS_PER_RELATION, SOURCE_IDS_LIMIT_METHOD)
    source_id = GRAPH_FIELD_SEP.join(source_ids)

    already_edge_source_set = set(already_source_ids)
    weight = sum(
        [dp["weight"] for dp in edges_data if dp.get("source_id") and dp["source_id"] not in already_edge_source_set]
        + already_weights
    )
    evidence_count = len({c for c in full_source_ids if c})
    weight = max(float(weight), float(evidence_count))

    unique_edges = {}
    for dp in edges_data:
        desc = dp.get("description")
        if desc and desc not in unique_edges:
            unique_edges[desc] = dp
    sorted_edges = sorted(
        unique_edges.values(), key=lambda x: (x.get("timestamp", 0), -len(x.get("description", "")))
    )
    description_list = combine_descriptions(
        already_description, [dp["description"] for dp in sorted_edges]
    )
    if not description_list:
        description_list = [f"Relation {src_id}~{tgt_id}"]

    relation_name = str((src_id, tgt_id))
    description, _llm_used = handle_entity_relation_summary(
        relation_name, description_list, llm_response, calls
    )

    all_keywords = set()
    for kw_str in [already_edge.get("keywords", "")] if already_edge else []:
        if kw_str:
            all_keywords.update(k.strip() for k in kw_str.split(",") if k.strip())
    for dp in edges_data:
        if dp.get("keywords"):
            all_keywords.update(k.strip() for k in dp["keywords"].split(",") if k.strip())
    combined_keywords = (
        ", ".join(sorted(all_keywords)) if all_keywords else (already_edge or {}).get("keywords", "")
    )

    file_paths_list, seen_paths = [], set()
    for fp in already_file_paths:
        if fp and fp not in seen_paths:
            file_paths_list.append(fp)
            seen_paths.add(fp)
    for dp in edges_data:
        fp = dp.get("file_path")
        if fp and fp not in seen_paths:
            file_paths_list.append(fp)
            seen_paths.add(fp)
    max_file_paths = file_path_limit if file_path_limit is not None else len(file_paths_list) + 1
    if len(file_paths_list) > max_file_paths:
        file_paths_list = file_paths_list[:max_file_paths]
        file_paths_list.append(f"...TRUNCATED...({SOURCE_IDS_LIMIT_METHOD})")
    file_path = GRAPH_FIELD_SEP.join(file_paths_list) if file_paths_list else UNKNOWN_SOURCE

    for need_insert_id in [src_id, tgt_id]:
        if not graph.has_node(need_insert_id):
            graph.upsert_node(
                need_insert_id,
                node_data={
                    "entity_id": need_insert_id,
                    "source_id": source_id,
                    "description": description,
                    "entity_type": "UNKNOWN",
                    "file_path": file_path,
                    "created_at": 0,
                    "truncate": "",
                },
            )

    truncate = "KEEP Old" if len(source_ids) < len(full_source_ids) else ""
    graph.upsert_edge(
        src_id,
        tgt_id,
        edge_data={
            "weight": weight,
            "description": description,
            "keywords": combined_keywords,
            "source_id": source_id,
            "file_path": file_path,
            "created_at": 0,
            "truncate": truncate,
        },
    )
    return graph.get_edge(src_id, tgt_id)


def normalise_source(value):
    return sorted(value.split(GRAPH_FIELD_SEP)) if value else []


def seed_json(seed):
    """Serialise a seed: edge keys are (src, tgt) tuples, JSON needs a string."""
    return {
        "nodes": seed["nodes"],
        "edges": {f"{src}\x00{tgt}": edge for (src, tgt), edge in seed["edges"].items()},
    }


def node_case(name, seed, entity, records, summary_response):
    graph = DictGraph(seed["nodes"], seed["edges"])
    calls = []
    merged = merge_nodes_then_upsert(entity, records, graph, summary_response, calls)
    return {
        "name": name,
        "kind": "node",
        "entity": entity,
        "seed": seed_json(seed),
        "records": records,
        "summary_response": summary_response,
        "expected": {
            "node": {k: (normalise_source(v) if k == "source_id" else v) for k, v in merged.items()},
            "summary_calls": [c["name"] for c in calls],
        },
    }


def edge_case(name, seed, src, tgt, records, summary_response):
    graph = DictGraph(seed["nodes"], seed["edges"])
    calls = []
    edge = merge_edges_then_upsert(src, tgt, records, graph, summary_response, calls)
    return {
        "name": name,
        "kind": "edge",
        "src": src,
        "tgt": tgt,
        "seed": seed_json(seed),
        "records": records,
        "summary_response": summary_response,
        "expected": {
            "edge": {k: (normalise_source(v) if k == "source_id" else v) for k, v in edge.items()},
            "endpoint_node": graph.get_node("BETA"),
            "summary_calls": [c["name"] for c in calls],
        },
    }


def main():
    cases = []

    # --- fresh node merge: majority type, timestamp-ordered descriptions -----
    records = [
        {"entity_name": "ACME", "entity_type": "organization", "description": "makes things",
         "source_id": "chunk-1", "file_path": "docs/a.md", "timestamp": 0},
        {"entity_name": "ACME", "entity_type": "organization", "description": "makes things",
         "source_id": "chunk-2", "file_path": "docs/a.md", "timestamp": 0},
        {"entity_name": "ACME", "entity_type": "person", "description": "tiny",
         "source_id": "chunk-3", "file_path": "docs/a.md", "timestamp": 0},
    ]
    cases.append(node_case(
        "fresh_node_majority_type_and_ordered_descriptions",
        {"nodes": {}, "edges": {}}, "ACME", records, "SUMMARY",
    ))

    # --- merge into an existing node -----------------------------------------
    seeded = {
        "nodes": {
            "ACME": {
                "entity_type": "organization",
                "description": "old description",
                "source_id": f"chunk-old{GRAPH_FIELD_SEP}chunk-1",
                "file_path": "docs/old.md",
                "created_at": 0,
                "truncate": "",
            }
        },
        "edges": {},
    }
    records = [
        {"entity_name": "ACME", "entity_type": "organization", "description": "new description",
         "source_id": "chunk-1", "file_path": "docs/a.md", "timestamp": 0},
        {"entity_name": "ACME", "entity_type": "organization", "description": "new description",
         "source_id": "chunk-2", "file_path": "docs/a.md", "timestamp": 0},
    ]
    cases.append(node_case(
        "existing_node_union_source_ids", seeded, "ACME", records, "SUMMARY",
    ))

    # --- tier-2 summary: long description forces the LLM call ---------------
    long_description = " ".join(f"word{i}" for i in range(1500))
    records = [
        {"entity_name": "ACME", "entity_type": "organization", "description": long_description,
         "source_id": "chunk-1", "file_path": "docs/a.md", "timestamp": 0},
    ]
    cases.append(node_case(
        "long_description_forces_summary", {"nodes": {}, "edges": {}}, "ACME", records,
        "LONG SUMMARY",
    ))

    # --- no description: the fallback fragment -------------------------------
    records = [
        {"entity_name": "ACME", "entity_type": "organization", "description": "",
         "source_id": "chunk-1", "file_path": "docs/a.md", "timestamp": 0},
    ]
    cases.append(node_case(
        "empty_description_uses_fallback", {"nodes": {}, "edges": {}}, "ACME", records, "SUMMARY",
    ))

    # --- file path cap --------------------------------------------------------
    records = [
        {"entity_name": "ACME", "entity_type": "organization", "description": "makes things",
         "source_id": "chunk-1", "file_path": "docs/a.md", "timestamp": 0},
        {"entity_name": "ACME", "entity_type": "organization", "description": "makes things",
         "source_id": "chunk-2", "file_path": "docs/b.md", "timestamp": 0},
        {"entity_name": "ACME", "entity_type": "organization", "description": "makes things",
         "source_id": "chunk-3", "file_path": "docs/c.md", "timestamp": 0},
    ]
    cases.append(node_case(
        "file_path_list_capped", {"nodes": {}, "edges": {}}, "ACME", records, "SUMMARY",
    ))

    # --- edge merge with existing edge and unknown endpoints -----------------
    seeded = {
        "nodes": {"ACME": {"entity_type": "organization", "description": "d", "source_id": "chunk-1"}},
        "edges": {
            ("ACME", "BETA"): {
                "weight": 1.0,
                "description": "edge description",
                "keywords": "owns",
                "source_id": "chunk-1",
                "file_path": "docs/a.md",
                "created_at": 0,
                "truncate": "",
            }
        },
    }
    records = [
        {"src_id": "ACME", "tgt_id": "BETA", "weight": 1.0, "description": "edge description",
         "source_id": "chunk-2", "order": 1, "keywords": "owns, controls", "file_path": "docs/a.md",
         "timestamp": 0},
    ]
    cases.append(edge_case(
        "edge_merge_keeps_evidence_weight", seeded, "ACME", "BETA", records, "SUMMARY",
    ))

    # --- edge merge, new edge, endpoints created -----------------------------
    records = [
        {"src_id": "ACME", "tgt_id": "BETA", "weight": 1.0, "description": "first",
         "source_id": "chunk-1", "order": 1, "keywords": "owns", "file_path": "docs/a.md",
         "timestamp": 0},
        {"src_id": "ACME", "tgt_id": "BETA", "weight": 1.0, "description": "first",
         "source_id": "chunk-2", "order": 1, "keywords": "controls", "file_path": "docs/b.md",
         "timestamp": 0},
    ]
    cases.append(edge_case(
        "edge_merge_creates_endpoints", {"nodes": {}, "edges": {}}, "ACME", "BETA", records,
        "SUMMARY",
    ))

    json.dump(cases, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
