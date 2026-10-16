#!/usr/bin/env python3
"""Generate community-report golden fixtures from the reference implementation.

Two reference surfaces are pinned here, both ported verbatim from
`refs/nano-graphrag/nano_graphrag/_op.py`:

* `_community_report_json_to_str` (:603-622) — the report JSON → markdown
  formatter whose bytes land in `community_reports.report_string` and, for
  coarse levels, in the `Reports` CSV of the next level's context.
* `_pack_single_community_describe` (:464-600) with
  `_pack_single_community_by_sub_communities` (:417-461) — the context block
  fed to the report LLM, together with `_utils.py`'s `list_of_list_to_csv`
  (:241), `enclose_string_with_quotes` (:232) and `truncate_list_by_token_size`
  (:169).

The `describe` cases need the real BPE (the budget arithmetic is measured in
tokens), so the script runs under the venv README §golden describes.

Usage:
    tests/.venvs/bin/python scripts/gen_report.py > tests/fixtures/report_golden.json
"""

import json
import sys
from numbers import Number

import tiktoken

TOK = tiktoken.encoding_for_model("gpt-4o")

GRAPH_FIELD_SEP = "<SEP>"


# ---------------------------------------------------------------------------
# Reference helpers (verbatim ports)
# ---------------------------------------------------------------------------


def token_len(text):
    return len(TOK.encode(text))


def enclose_string_with_quotes(content):
    """`_utils.py:232` — numbers stay bare, text is wrapped in quotes."""
    if isinstance(content, Number):
        return str(content)
    content = str(content)
    content = content.strip().strip("'").strip('"')
    return f'"{content}"'


def list_of_list_to_csv(data):
    """`_utils.py:241` — `",\t"` join, `"\n"` rows."""
    return "\n".join(
        [
            ",\t".join([f"{enclose_string_with_quotes(data_dd)}" for data_dd in data_d])
            for data_d in data
        ]
    )


def truncate_list_by_token_size(list_data, key, max_token_size):
    """`_utils.py:169` — stop before the row that would overflow."""
    if max_token_size <= 0:
        return []
    tokens = 0
    for i, data in enumerate(list_data):
        tokens += token_len(key(data)) + 1
        if tokens > max_token_size:
            return list_data[:i]
    return list_data


class DictGraph:
    """Minimal graph mock: the reference reads nodes, edges and degrees.

    Degrees follow the in-memory backend the port's tests run against: a node's
    degree is the number of edges touching it (both directions count) and an
    edge's degree is the sum of its two endpoints' degrees.
    """

    def __init__(self, nodes=None, edges=None):
        self.nodes = dict(nodes or {})
        self.edges = {}
        for (src, tgt), data in (edges or {}).items():
            self.edges.setdefault((src, tgt), {}).update(data)

    def get_node(self, node_id):
        return self.nodes.get(node_id)

    def get_edge(self, src, tgt):
        return self.edges.get((src, tgt))

    def node_degree(self, node_id):
        return sum(1 for (a, b) in self.edges if a == node_id or b == node_id)

    def edge_degree(self, src, tgt):
        return self.node_degree(src) + self.node_degree(tgt)


def community_report_json_to_str(parsed_output):
    """`_community_report_json_to_str` (`_op.py:603-622`)."""
    title = parsed_output.get("title", "Report")
    summary = parsed_output.get("summary", "")
    findings = parsed_output.get("findings", [])

    def finding_summary(finding):
        if isinstance(finding, str):
            return finding
        return finding.get("summary")

    def finding_explanation(finding):
        if isinstance(finding, str):
            return ""
        return finding.get("explanation")

    report_sections = "\n\n".join(
        f"## {finding_summary(f)}\n\n{finding_explanation(f)}" for f in findings
    )
    return f"# {title}\n\n{summary}\n\n{report_sections}"


def pack_single_community_by_sub_communities(
    community, max_token_size, already_reports
):
    """`_pack_single_community_by_sub_communities` (`_op.py:417-461`)."""
    all_sub_communities = [
        already_reports[k] for k in community["sub_communities"] if k in already_reports
    ]
    all_sub_communities = sorted(
        all_sub_communities, key=lambda x: x["occurrence"], reverse=True
    )

    may_trun_all_sub_communities = truncate_list_by_token_size(
        all_sub_communities, key=lambda x: x["report_string"], max_token_size=max_token_size
    )
    sub_fields = ["id", "report", "rating", "importance"]
    sub_communities_describe = list_of_list_to_csv(
        [sub_fields]
        + [
            [
                i,
                c["report_string"],
                c["report_json"].get("rating", -1),
                c["occurrence"],
            ]
            for i, c in enumerate(may_trun_all_sub_communities)
        ]
    )
    already_nodes = []
    already_edges = []
    for c in may_trun_all_sub_communities:
        already_nodes.extend(c["nodes"])
        already_edges.extend([tuple(e) for e in c["edges"]])

    return (
        sub_communities_describe,
        token_len(sub_communities_describe),
        set(already_nodes),
        set(already_edges),
    )


def pack_single_community_describe(
    knwoledge_graph_inst,
    community,
    max_token_size=12000,
    already_reports=None,
    force_to_use_sub_communities=False,
):
    """`_pack_single_community_describe` (`_op.py:464-600`), awaits removed.

    The reference's `already_reports: dict = {}` default is a mutable one; it is
    rebound to a fresh dict here because the function never writes to it and
    ruff rightly rejects the shared-default form.
    """
    already_reports = {} if already_reports is None else already_reports
    # 1. 准备原始数据
    nodes_in_order = sorted(community["nodes"])
    edges_in_order = sorted(community["edges"], key=lambda x: x[0] + x[1])

    nodes_data = [knwoledge_graph_inst.get_node(n) for n in nodes_in_order]
    edges_data = [
        knwoledge_graph_inst.get_edge(src, tgt) for src, tgt in edges_in_order
    ]

    # 2. 定义模板和固定开销
    final_template = """-----Reports-----
```csv
{reports}
```
-----Entities-----
```csv
{entities}
```
-----Relationships-----
```csv
{relationships}
```"""
    base_template_tokens = token_len(
        final_template.format(reports="", entities="", relationships="")
    )
    remaining_budget = max_token_size - base_template_tokens

    # 3. 处理子社区报告
    report_describe = ""
    contain_nodes = set()
    contain_edges = set()

    truncated = len(nodes_in_order) > 100 or len(edges_in_order) > 100

    need_to_use_sub_communities = (
        truncated and community["sub_communities"] and already_reports
    )

    if need_to_use_sub_communities or force_to_use_sub_communities:
        (
            report_describe,
            report_size,
            contain_nodes,
            contain_edges,
        ) = pack_single_community_by_sub_communities(
            community, remaining_budget, already_reports
        )
        remaining_budget = max(0, remaining_budget - report_size)

    # 4. 准备节点和边数据（过滤子社区已包含的）
    def format_row(row):
        return ",".join('"{}"'.format(str(item).replace('"', '""')) for item in row)

    node_fields = ["id", "entity", "type", "description", "degree"]
    edge_fields = ["id", "source", "target", "description", "rank"]

    node_degrees = [knwoledge_graph_inst.node_degree(n) for n in nodes_in_order]
    edge_degrees = [
        knwoledge_graph_inst.edge_degree(src, tgt) for src, tgt in edges_in_order
    ]

    nodes_list_data = [
        [
            i,
            name,
            data.get("entity_type", "UNKNOWN"),
            data.get("description", "UNKNOWN"),
            node_degrees[i],
        ]
        for i, (name, data) in enumerate(zip(nodes_in_order, nodes_data))
        if name not in contain_nodes
    ]

    edges_list_data = [
        [
            i,
            edge[0],
            edge[1],
            data.get("description", "UNKNOWN"),
            edge_degrees[i],
        ]
        for i, (edge, data) in enumerate(zip(edges_in_order, edges_data))
        if (edge[0], edge[1]) not in contain_edges
    ]

    nodes_list_data.sort(key=lambda x: x[-1], reverse=True)
    edges_list_data.sort(key=lambda x: x[-1], reverse=True)

    # 5. 动态分配预算
    header_tokens = token_len(
        list_of_list_to_csv([node_fields]) + "\n" + list_of_list_to_csv([edge_fields])
    )

    data_budget = max(0, remaining_budget - header_tokens)
    total_items = len(nodes_list_data) + len(edges_list_data)
    node_ratio = len(nodes_list_data) / max(1, total_items)
    edge_ratio = 1 - node_ratio

    nodes_final = truncate_list_by_token_size(
        nodes_list_data, key=format_row, max_token_size=int(data_budget * node_ratio)
    )
    edges_final = truncate_list_by_token_size(
        edges_list_data, key=format_row, max_token_size=int(data_budget * edge_ratio)
    )

    # 6. 组装最终输出
    nodes_describe = list_of_list_to_csv([node_fields] + nodes_final)
    edges_describe = list_of_list_to_csv([edge_fields] + edges_final)

    return final_template.format(
        reports=report_describe, entities=nodes_describe, relationships=edges_describe
    )


# ---------------------------------------------------------------------------
# Cases
# ---------------------------------------------------------------------------


def json_case(name, parsed, note):
    return {
        "name": name,
        "kind": "json_to_str",
        "note": note,
        "input": parsed,
        "expected": community_report_json_to_str(parsed),
    }


def describe_case(name, community, graph, max_token_size, already_reports, force, note):
    return {
        "name": name,
        "kind": "describe",
        "note": note,
        "community": community,
        "graph": {
            "nodes": graph["nodes"],
            "edges": {f"{src}\x00{tgt}": data for (src, tgt), data in graph["edges"].items()},
        },
        "max_token_size": max_token_size,
        "force_to_use_sub_communities": force,
        "already_reports": already_reports,
        "expected": pack_single_community_describe(
            DictGraph(graph["nodes"], graph["edges"]),
            community,
            max_token_size=max_token_size,
            already_reports=already_reports,
            force_to_use_sub_communities=force,
        ),
    }


# A long, distinct description per node so the truncation case has a wide
# margin: a couple of tokens of tokenizer drift cannot change which rows fit.
LONG_A = (
    "Acme Corporation designs and assembles collaborative warehouse robots "
    "for mid sized distribution centres across the alpine region, and it "
    "maintains a dedicated research group focused on safe motion planning."
)
LONG_B = (
    "Beta Labs builds small autonomous delivery drones that navigate urban "
    "corridors without a permanent flight corridor, and it publishes its "
    "trajectory planner under a permissive licence for academic reuse."
)
LONG_C = (
    "Geneva hosts both companies inside its industrial district, and the local "
    "university runs the safety certification programme that both of them "
    "depend on for cross border shipments of their assembled units."
)
LONG_D = (
    "Zurich supplies the precision actuators that both assembly lines fit "
    "into their joints, and the supplier audits every batch against the "
    "tolerance table before it ships the parts to the two assembly plants."
)
LONG_E_ACME_BETA = (
    "Acme Corporation and Beta Labs compete for the same mid sized warehouse "
    "accounts, and the contest has narrowed twice in the last eighteen months "
    "after each of them cut its list price on the flagship cobot class."
)
LONG_E_BETA_GENEVA = (
    "Beta Labs certifies its drones with the Geneva programme before any unit "
    "may leave the district, which adds six weeks to every delivery season and "
    "has kept the two rivals out of each other's certified corridors."
)

NODE_PROPS = {
    "ACME": {"entity_type": "ORGANIZATION", "description": LONG_A},
    "BETA": {"entity_type": "ORGANIZATION", "description": LONG_B},
    "GENEVA": {"entity_type": "LOCATION", "description": LONG_C},
    "ZURICH": {"entity_type": "LOCATION", "description": LONG_D},
    # Quote boundary: `enclose_string_with_quotes` strips leading/trailing
    # apostrophes and double quotes before wrapping (`_utils.py:236-238`).
    "OMEGA": {"entity_type": "CONCEPT", "description": "\"Pinned\" quotes"},
    # `entity_type`/`description` absent → the reference substitutes UNKNOWN.
    "UNKNOWN_NODE": {},
}
EDGE_PROPS = {
    ("ACME", "BETA"): {"description": LONG_E_ACME_BETA},
    ("BETA", "GENEVA"): {"description": LONG_E_BETA_GENEVA},
    ("ZURICH", "ACME"): {"description": 'supplies "actuators", batch audited'},
    ("BETA", "ZURICH"): {"description": "sources actuators"},
}
GRAPH = {"nodes": NODE_PROPS, "edges": EDGE_PROPS}

# Quote/newline boundary fixture: a title with quotes, a summary with an
# embedded quote and a trailing space, multi-line explanations and a numeric
# rating. `rating` is not rendered by the formatter at all — it is read by
# `report_rating` for global search ordering — so the case pins that it does
# not leak into the markdown.
QUOTE_BOUNDARY = {
    "title": "The \"bridge\" community",
    "summary": 'ACME and BETA "compete" for the same accounts. ',
    "findings": [
        {"summary": "Headline with \"quotes\"", "explanation": "Line one.\nLine two."},
        {"summary": "Plain", "explanation": ""},
    ],
    "rating": 7.5,
    "rating_explanation": "Because it matters.",
}

# A finding object without `explanation`: the reference's
# `finding_explanation` is `finding.get("explanation")` with no default
# (`_op.py:614-617`), so the f-string prints `None`.
MISSING_EXPLANATION = {
    "title": "Malformed finding",
    "summary": "The LLM dropped a key.",
    "findings": [{"summary": "Only a summary", "explanation": "Present."}, {"summary": "Bare"}],
    "rating": 1,
}

MULTI_FINDING = {
    "title": "Community",
    "summary": "Two clusters linked.",
    "findings": [
        {"summary": "Bridge", "explanation": "A1 and B1 talk."},
        {"summary": "Churn", "explanation": "Both clusters lose members."},
    ],
    "rating": 6,
}

STRING_FINDING = {
    "title": "Community",
    "summary": "Findings are plain strings.",
    "findings": ["plain string finding", 'another "quoted" one'],
    "rating": 2,
}

# Level-1 community over the shared graph, ordered node/edge lists.
LEVEL1 = {
    "level": 1,
    "title": "Alpine robotics",
    "nodes": ["ZURICH", "BETA", "ACME", "GENEVA", "OMEGA"],
    "edges": [["BETA", "ZURICH"], ["ACME", "BETA"], ["ZURICH", "ACME"], ["BETA", "GENEVA"]],
    # `chunk_ids` is part of `SingleCommunitySchema`; the describe step never
    # reads it, but the Rust side deserializes the whole schema.
    "chunk_ids": ["chunk-1"],
    "occurrence": 12.0,
    "sub_communities": [],
}

# Level-2 community whose sub-community already carries a report: the Reports
# CSV quotes that report string verbatim (quotes survive; only leading and
# trailing ones are stripped), and the covered node/edge are filtered out of
# the Entities/Relationships CSVs.
SUB_COMMUNITY = {
    "level": 0,
    "title": "Beta cluster",
    "nodes": ["BETA"],
    "edges": [["BETA", "ZURICH"]],
    "chunk_ids": ["chunk-1"],
    "occurrence": 4.0,
    "sub_communities": [],
    "report_string": '# Beta "cluster"\n\nBeta sources actuators.',
    "report_json": {"rating": 7.5},
}
LEVEL2 = {
    "level": 1,
    "title": "Alpine robotics, coarse",
    "nodes": ["ZURICH", "BETA", "ACME", "GENEVA", "OMEGA", "UNKNOWN_NODE"],
    "edges": [["ACME", "BETA"], ["BETA", "ZURICH"]],
    "chunk_ids": ["chunk-1", "chunk-2"],
    "occurrence": 3.0,
    "sub_communities": ["0"],
}


def main():
    cases = [
        json_case(
            "multiple_findings",
            MULTI_FINDING,
            "Several object findings join with a blank line.",
        ),
        json_case(
            "string_findings",
            STRING_FINDING,
            "A string finding renders its explanation as the empty string.",
        ),
        json_case(
            "quote_and_rating_boundaries",
            QUOTE_BOUNDARY,
            "Quotes, newlines and a trailing space survive; `rating` never "
            "reaches the markdown.",
        ),
        json_case(
            "missing_explanation_key",
            MISSING_EXPLANATION,
            "`finding.get('explanation')` has no default, so a finding object "
            "without the key renders the word `None`.",
        ),
        describe_case(
            "describe_orders_entities_and_relationships_by_degree",
            LEVEL1,
            GRAPH,
            12000,
            {},
            False,
            "Rows sort by descending degree (stable), not by input order; the "
            "id column keeps the sorted position the row had before the sort.",
        ),
        describe_case(
            "describe_uses_sub_community_reports",
            LEVEL2,
            GRAPH,
            12000,
            {"0": SUB_COMMUNITY},
            True,
            "The covered node/edge leave the entity and relationship CSVs.",
        ),
        describe_case(
            "describe_truncates_to_the_token_budget",
            LEVEL1,
            GRAPH,
            263,
            {},
            False,
            "The budget lands mid-plateau, so a couple of tokens of tokenizer "
            "drift cannot change which rows fit: node rows measure 45/45/48 "
            "tokens (91 kept, 140 rejected, budget 115) and edge rows 52/16/51 "
            "(69 kept, 120 rejected, budget 92).",
        ),
    ]

    json.dump(
        {
            "generator": "tests/.venvs/bin/python scripts/gen_report.py > "
            "tests/fixtures/report_golden.json",
            "reference": {
                "module": "nano_graphrag/_op.py",
                "functions": [
                    "_community_report_json_to_str (603-622)",
                    "_pack_single_community_describe (464-600)",
                    "_pack_single_community_by_sub_communities (417-461)",
                ],
                "helpers": [
                    "_utils.py::list_of_list_to_csv (241)",
                    "_utils.py::enclose_string_with_quotes (232)",
                    "_utils.py::truncate_list_by_token_size (169)",
                ],
                "encoding": "o200k_base",
            },
            "cases": cases,
        },
        sys.stdout,
        ensure_ascii=False,
        indent=1,
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
