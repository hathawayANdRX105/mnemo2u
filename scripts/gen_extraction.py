#!/usr/bin/env python3
"""Generate extraction-parser golden fixtures from the reference algorithm.

The functions below are copied from
`refs/nano-graphrag/nano_graphrag/_op.py` (`_handle_single_entity_extraction`
:138-156, `_handle_single_relationship_extraction` :159-179, the record loop in
`extract_entities` :349-394) plus the `_utils.py` helpers they use. Only the
standard library is needed, so this runs without the reference package.

Usage:
    tests/.venvs/bin/python scripts/gen_extraction.py > tests/fixtures/extraction_golden.json
"""

import html
import json
import re
import sys

GRAPH_FIELD_SEP = "<SEP>"
TUPLE_DELIMITER = "<|>"
RECORD_DELIMITER = "##"
COMPLETION_DELIMITER = "<|COMPLETE|>"


def clean_str(value):
    if not isinstance(value, str):
        return value
    result = html.unescape(value.strip())
    return re.sub(r"[\x00-\x1f\x7f-\x9f]", "", result)


def split_string_by_multi_markers(content, markers):
    if not markers:
        return [content]
    results = re.split("|".join(re.escape(marker) for marker in markers), content)
    return [r.strip() for r in results if r.strip()]


def is_float_regex(value):
    return bool(re.match(r"^[-+]?[0-9]*\.?[0-9]+$", value))


def handle_single_entity_extraction(record_attributes, chunk_key):
    if len(record_attributes) < 4 or record_attributes[0] != '"entity"':
        return None
    entity_name = clean_str(record_attributes[1].upper())
    if not entity_name.strip():
        return None
    return {
        "entity_name": entity_name,
        "entity_type": clean_str(record_attributes[2].upper()),
        "description": clean_str(record_attributes[3]),
        "source_id": chunk_key,
    }


def handle_single_relationship_extraction(record_attributes, chunk_key):
    if len(record_attributes) < 5 or record_attributes[0] != '"relationship"':
        return None
    source = clean_str(record_attributes[1].upper())
    target = clean_str(record_attributes[2].upper())
    weight = float(record_attributes[-1]) if is_float_regex(record_attributes[-1]) else 1.0
    return {
        "src_id": source,
        "tgt_id": target,
        "weight": weight,
        "description": clean_str(record_attributes[3]),
        "source_id": chunk_key,
        "order": 1,
    }


def parse(final_result, chunk_key):
    records = split_string_by_multi_markers(
        final_result, [RECORD_DELIMITER, COMPLETION_DELIMITER]
    )
    maybe_nodes = {}
    maybe_edges = {}
    for record in records:
        record = re.search(r"\((.*)\)", record)
        if record is None:
            continue
        record = record.group(1)
        record_attributes = split_string_by_multi_markers(record, [TUPLE_DELIMITER])
        entity = handle_single_entity_extraction(record_attributes, chunk_key)
        if entity is not None:
            maybe_nodes.setdefault(entity["entity_name"], []).append(entity)
            continue
        relation = handle_single_relationship_extraction(record_attributes, chunk_key)
        if relation is not None:
            key = tuple(sorted((relation["src_id"], relation["tgt_id"])))
            maybe_edges.setdefault(key, []).append(relation)
    return {
        "nodes": [[name, records] for name, records in maybe_nodes.items()],
        "edges": [[list(key), records] for key, records in maybe_edges.items()],
    }


CASES = [
    {
        "name": "reference_example_quoted",
        "chunk_key": "chunk-aaa",
        "raw": (
            '("entity"<|>"Alex"<|>"person"<|>"Alex is a character.")##'
            '("entity"<|>"Taylor"<|>"person"<|>"Taylor is certain.")##'
            '("relationship"<|>"Alex"<|>"Taylor"<|>"Alex observes Taylor."<|>7)##'
            '("entity"<|>"The Device"<|>"technology"<|>"Central to the story.")<|COMPLETE|>'
        ),
    },
    {
        "name": "unquoted_fields_and_spacing",
        "chunk_key": "chunk-bbb",
        "raw": (
            '( "entity"<|>  acme corp <|> organization <|> makes things )  ##\n'
            '("relationship" <|> acme corp <|> beta labs <|> owns <|> 3.25 )'
        ),
    },
    {
        "name": "malformed_and_multiline",
        "chunk_key": "chunk-ccc",
        "raw": (
            '("entity"<|>"A"<|>"B")##'  # too few attributes
            '("entity"<|>"  "<|>"B"<|>"d")##'  # empty name after cleaning
            '("entity"<|>"C"<|>"D"<|>"first line\nsecond line")##'  # newline inside the record
            '("relationship"<|>"C"<|>"D"<|>"desc"<|>not-a-number)'
        ),
    },
    {
        "name": "nested_parentheses",
        "chunk_key": "chunk-ddd",
        "raw": '("entity"<|>"X (the founder)"<|>"person"<|>"X (born 1970) leads.")',
    },
    {
        "name": "escaped_entities_and_controls",
        "chunk_key": "chunk-eee",
        "raw": '("entity"<|>"A&amp;B\x07"<|>"org"<|>"R&amp;D unit")',
    },
]


def main():
    payload = []
    for case in CASES:
        expected = parse(case["raw"], case["chunk_key"])
        payload.append(
            {
                "name": case["name"],
                "chunk_key": case["chunk_key"],
                "raw": case["raw"],
                "expected": expected,
            }
        )
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
