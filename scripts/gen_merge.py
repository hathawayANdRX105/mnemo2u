#!/usr/bin/env python3
"""Generate merge golden fixtures from the reference algorithm.

The functions below are copied from
`refs/nano-graphrag/nano_graphrag/_op.py` (`_merge_nodes_then_upsert` :182-227,
`_merge_edges_then_upsert` :230-279, `_handle_entity_relation_summary` :111-135)
with a dict-backed graph mock standing in for the storage layer.

Notes on set ordering: the reference builds `source_id` by joining a Python
`set`, whose iteration order is implementation-defined. The fixture therefore
records `source_id` as a *sorted list*, and the Rust test compares sets.

Usage:
    tests/.venvs/bin/python scripts/gen_merge.py > tests/fixtures/merge_golden.json
"""

import copy
import json
import re
import sys

GRAPH_FIELD_SEP = "<SEP>"


def clean_str(value):
    return value


def split_string_by_multi_markers(content, markers):
    if not markers:
        return [content]
    results = re.split("|".join(re.escape(marker) for marker in markers), content)
    return [r.strip() for r in results if r.strip()]


class DictGraph:
    def __init__(self, nodes=None, edges=None):
        # Deep copy: the caller's dicts are recorded verbatim as the case
        # `seed`, and `upsert_*` mutates inner node/edge dicts in place —
        # without the copy the fixture would capture post-merge state as the
        # "before" state and the Rust test would seed its graph with it.
        self.nodes = copy.deepcopy(nodes or {})
        self.edges = copy.deepcopy(edges or {})

    def get_node(self, node_id):
        return self.nodes.get(node_id)

    def upsert_node(self, node_id, node_data):
        self.nodes.setdefault(node_id, {}).update(node_data)

    def has_edge(self, src, tgt):
        return (src, tgt) in self.edges

    def get_edge(self, src, tgt):
        return self.edges.get((src, tgt))

    def upsert_edge(self, src, tgt, edge_data):
        self.edges.setdefault((src, tgt), {}).update(edge_data)

    def has_node(self, node_id):
        return node_id in self.nodes


def handle_entity_relation_summary(name, description, summary_max_tokens, llm_max_tokens, llm_response, calls):
    # token counting is approximated with whitespace splitting: the fixture
    # only needs the < threshold / >= threshold decision, and the Rust side
    # scripts its mock with the same response.
    tokens = re.findall(r"\S+", description)
    if len(tokens) < summary_max_tokens:
        return description, calls
    calls.append({"name": name, "description_list": description.split(GRAPH_FIELD_SEP)})
    return llm_response, calls


def merge_nodes_then_upsert(entity_name, nodes_data, graph, llm_response, calls):
    already_entitiy_types = []
    already_source_ids = []
    already_description = []
    already_node = graph.get_node(entity_name)
    if already_node is not None:
        already_entitiy_types.append(already_node["entity_type"])
        already_source_ids.extend(split_string_by_multi_markers(already_node["source_id"], [GRAPH_FIELD_SEP]))
        already_description.append(already_node["description"])

    from collections import Counter

    # `max` keeps the reference's tie-break (first of the highest count).
    entity_type = max(
        Counter([dp["entity_type"] for dp in nodes_data] + already_entitiy_types).items(),
        key=lambda x: x[1],
    )[0]
    description = GRAPH_FIELD_SEP.join(
        sorted(set([dp["description"] for dp in nodes_data] + already_description))
    )
    source_id = GRAPH_FIELD_SEP.join(set([dp["source_id"] for dp in nodes_data] + already_source_ids))
    description, calls = handle_entity_relation_summary(
        entity_name, description, 500, 32768, llm_response, calls
    )
    node_data = {
        "entity_type": entity_type,
        "description": description,
        "source_id": source_id,
    }
    graph.upsert_node(entity_name, node_data=node_data)
    node_data["entity_name"] = entity_name
    return node_data, calls


def merge_edges_then_upsert(src_id, tgt_id, edges_data, graph, llm_response, calls):
    already_weights = []
    already_source_ids = []
    already_description = []
    already_order = []
    if graph.has_edge(src_id, tgt_id):
        already_edge = graph.get_edge(src_id, tgt_id)
        already_weights.append(already_edge["weight"])
        already_source_ids.extend(split_string_by_multi_markers(already_edge["source_id"], [GRAPH_FIELD_SEP]))
        already_description.append(already_edge["description"])
        already_order.append(already_edge.get("order", 1))

    order = min([dp.get("order", 1) for dp in edges_data] + already_order)
    weight = sum([dp["weight"] for dp in edges_data] + already_weights)
    description = GRAPH_FIELD_SEP.join(
        sorted(set([dp["description"] for dp in edges_data] + already_description))
    )
    source_id = GRAPH_FIELD_SEP.join(set([dp["source_id"] for dp in edges_data] + already_source_ids))
    for need_insert_id in [src_id, tgt_id]:
        if not graph.has_node(need_insert_id):
            graph.upsert_node(
                need_insert_id,
                node_data={
                    "source_id": source_id,
                    "description": description,
                    "entity_type": '"UNKNOWN"',
                },
            )
    relation_name = str((src_id, tgt_id))
    description, calls = handle_entity_relation_summary(
        relation_name, description, 500, 32768, llm_response, calls
    )
    graph.upsert_edge(
        src_id,
        tgt_id,
        edge_data={
            "weight": weight,
            "description": description,
            "source_id": source_id,
            "order": order,
        },
    )
    return calls


def normalise_source(value):
    return sorted(value.split(GRAPH_FIELD_SEP)) if value else []


def main():
    cases = []

    # --- case: fresh node merge -------------------------------------------------
    graph = DictGraph()
    nodes = [
        {"entity_name": "ACME", "entity_type": "ORGANIZATION", "description": "makes things", "source_id": "chunk-1"},
        {"entity_name": "ACME", "entity_type": "ORGANIZATION", "description": "makes things", "source_id": "chunk-2"},
        {"entity_name": "ACME", "entity_type": "PERSON", "description": "tiny", "source_id": "chunk-3"},
    ]
    calls = []
    merged, calls = merge_nodes_then_upsert("ACME", nodes, graph, "SUMMARY", calls)
    cases.append({
        "name": "fresh_node_majority_type_and_sorted_descriptions",
        "kind": "node",
        "entity": "ACME",
        "seed": {"nodes": {}, "edges": {}},
        "records": nodes,
        "summary_response": "SUMMARY",
        "expected": {
            "node": {k: (normalise_source(v) if k == "source_id" else v) for k, v in merged.items() if k != "source_id"},
            "source_id": normalise_source(merged["source_id"]),
            "summary_calls": [c["name"] for c in calls],
        },
    })

    # --- case: merge into an existing node -------------------------------------
    seeded = {
        "nodes": {
            "ACME": {
                "entity_type": "ORG",
                "description": "old description",
                "source_id": f"chunk-old{GRAPH_FIELD_SEP}chunk-1",
            }
        },
        "edges": {},
    }
    graph = DictGraph(seeded["nodes"], seeded["edges"])
    nodes = [
        {"entity_name": "ACME", "entity_type": "ORG", "description": "new description", "source_id": "chunk-1"},
        {"entity_name": "ACME", "entity_type": "ORG", "description": "new description", "source_id": "chunk-2"},
    ]
    calls = []
    merged, calls = merge_nodes_then_upsert("ACME", nodes, graph, "SUMMARY", calls)
    cases.append({
        "name": "existing_node_union_source_ids",
        "kind": "node",
        "entity": "ACME",
        "seed": seeded,
        "records": nodes,
        "summary_response": "SUMMARY",
        "expected": {
            "node": {k: v for k, v in merged.items() if k != "source_id"},
            "source_id": normalise_source(merged["source_id"]),
            "summary_calls": [c["name"] for c in calls],
        },
    })

    # --- case: edge merge with existing edge and unknown endpoints -------------
    seeded = {
        "nodes": {"ACME": {"entity_type": "ORG", "description": "d", "source_id": "chunk-1"}},
        "edges": {
            ("ACME", "BETA"): {
                "weight": 2.0,
                "description": "old relation",
                "source_id": "chunk-9",
                "order": 3,
            }
        },
    }
    graph = DictGraph(seeded["nodes"], seeded["edges"])
    edges = [
        {"src_id": "ACME", "tgt_id": "BETA", "weight": 1.5, "description": "new relation", "source_id": "chunk-2", "order": 1},
        {"src_id": "ACME", "tgt_id": "BETA", "weight": 2.0, "description": "third relation", "source_id": "chunk-3", "order": 2},
    ]
    calls = []
    calls = merge_edges_then_upsert("ACME", "BETA", edges, graph, "SHORT", calls)
    edge = graph.get_edge("ACME", "BETA")
    beta = graph.get_node("BETA")
    cases.append({
        "name": "edge_merge_weight_order_endpoint_fill",
        "kind": "edge",
        "src": "ACME",
        "tgt": "BETA",
        "seed": {
            "nodes": seeded["nodes"],
            "edges": {"ACME\u0000BETA": seeded["edges"][("ACME", "BETA")]},
        },
        "records": edges,
        "summary_response": "SHORT",
        "expected": {
            "edge": {k: v for k, v in edge.items() if k != "source_id"},
            "edge_source_id": normalise_source(edge["source_id"]),
            "endpoint_node": beta,
            "summary_calls": [c["name"] for c in calls],
        },
    })

    # --- case: summary threshold triggers --------------------------------------
    long_text = " ".join(f"token{i}" for i in range(600))
    graph = DictGraph()
    nodes = [{"entity_name": "BIG", "entity_type": "CONCEPT", "description": long_text, "source_id": "chunk-1"}]
    calls = []
    merged, calls = merge_nodes_then_upsert("BIG", nodes, graph, "COMPRESSED", calls)
    cases.append({
        "name": "long_description_triggers_summary",
        "kind": "node",
        "entity": "BIG",
        "seed": {"nodes": {}, "edges": {}},
        "records": nodes,
        "summary_response": "COMPRESSED",
        "expected": {
            "node": {k: v for k, v in merged.items() if k != "source_id"},
            "source_id": normalise_source(merged["source_id"]),
            "summary_calls": [c["name"] for c in calls],
        },
    })

    json.dump(cases, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
