#!/usr/bin/env python3
"""Generate extraction-parser golden fixtures from the LightRAG algorithm.

The functions below are copied verbatim from `refs/LightRAG/lightrag`:
`_handle_single_entity_extraction` (operate.py:710),
`_handle_single_relationship_extraction` (operate.py:774),
`_normalize_text_extraction_record_attributes` (operate.py:854),
`_process_extraction_result` (operate.py:1508), plus the utils they use
(`sanitize_text_for_encoding` :5973, `normalize_extracted_info` :5838,
`sanitize_and_normalize_extracted_text` :5808, `normalize_entity_name` :5833,
`split_string_by_multi_markers` :3997). Only the standard library is needed.

Usage:
    python3 scripts/gen_extraction.py > tests/fixtures/extraction_golden.json
"""

import html
import json
import re
import sys
from collections import defaultdict

TUPLE_DELIMITER = "<|#|>"
COMPLETION_DELIMITER = "<|COMPLETE|>"
GRAPH_FIELD_SEP = "<SEP>"
DEFAULT_ENTITY_NAME_MAX_LENGTH = 256
_RESERVED_ENTITY_TYPES = frozenset({"__proto__", "constructor", "prototype"})

# ---------------------------------------------------------------- utils.py


def _control_char_pattern():
    # C0/C1 controls plus the Unicode separators the reference strips.
    return re.compile(r"[\x00-\x1f\x7f-\x9f]")


_SURROGATE_PATTERN = re.compile(r"[\ud800-\udfff]")
_CONTROL_CHAR_PATTERN_ALL = _control_char_pattern()


def sanitize_text_for_encoding(text: str, replacement_char: str = "", *, strip: bool = True) -> str:
    if strip:
        text = text.strip()
    if not text:
        return text
    text = html.unescape(text)
    text = _SURROGATE_PATTERN.sub(replacement_char, text)
    text = _CONTROL_CHAR_PATTERN_ALL.sub(replacement_char, text)
    return text.strip() if strip else text


def _remove_space_between(text: str, first: str, second: str) -> str:
    r"""`(?<=first)\s+(?=second)` removal.

    The reference uses lookaround; Python 3.14 still supports it, so this keeps
    the reference semantics verbatim (including adjacent pairs like
    `A B C` -> `ABC`).
    """
    pattern = re.compile(f"(?<={first})\\s+(?={second})")
    return pattern.sub("", text)


def normalize_extracted_info(name: str, remove_inner_quotes=False) -> str:
    text = sanitize_text_for_encoding(name)
    if not text:
        return ""
    text = re.sub(r"</p\s*>|<p\s*>|<p/>", "", text, flags=re.IGNORECASE)
    text = re.sub(r"</br\s*>|<br\s*>|<br/>", "", text, flags=re.IGNORECASE)
    text = text.translate(
        str.maketrans(
            "ＡＢＣＤＥＦＧＨＩＪＫＬＭＮＯＰＱＲＳＴＵＶＷＸＹＺａｂｃｄｅｆｇｈｉｊｋｌｍｎｏｐｑｒｓｔｕｖｗｘｙｚ",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        )
    )
    text = text.translate(str.maketrans("０１２３４５６７８９", "0123456789"))
    text = text.replace("－", "-")
    text = text.replace("＋", "+")
    text = text.replace("／", "/")
    text = text.replace("＊", "*")
    text = text.replace("（", "(").replace("）", ")")
    text = text.replace("—", "-").replace("－", "-")
    text = text.replace("　", " ")
    text = _remove_space_between(text, r"[\u4e00-\u9fa5]", r"[\u4e00-\u9fa5]")
    text = _remove_space_between(
        text, r"[\u4e00-\u9fa5]", r"[a-zA-Z0-9\(\)\[\]@#$%!&\*\-=+_]"
    )
    text = _remove_space_between(
        text, r"[a-zA-Z0-9\(\)\[\]@#$%!&\*\-=+_]", r"[\u4e00-\u9fa5]"
    )

    if len(text) >= 2:
        if text.startswith('"') and text.endswith('"'):
            inner = text[1:-1]
            if '"' not in inner:
                text = inner
        if text.startswith("'") and text.endswith("'"):
            inner = text[1:-1]
            if "'" not in inner:
                text = inner
        if text.startswith("“") and text.endswith("”"):
            inner = text[1:-1]
            if "“" not in inner and "”" not in inner:
                text = inner
        if text.startswith("‘") and text.endswith("’"):
            inner = text[1:-1]
            if "‘" not in inner and "’" not in inner:
                text = inner
        if text.startswith("《") and text.endswith("》"):
            inner = text[1:-1]
            if "《" not in inner and "》" not in inner:
                text = inner

    if remove_inner_quotes:
        text = text.replace("“", "").replace("”", "").replace("‘", "").replace("’", "")
        text = re.sub(r"['\"]+(?=[\u4e00-\u9fa5])", "", text)
        text = re.sub(r"(?<=[\u4e00-\u9fa5])['\"]+", "", text)
        text = text.replace("\u00a0", " ")
        text = re.sub(r"(?<=[^\d])\u202F", " ", text)

    text = text.strip()
    if len(text) < 3 and re.match(r"^[0-9]+$", text):
        return ""

    def should_filter_by_dots(value: str) -> bool:
        return all(c.isdigit() or c == "." for c in value) and "." in value

    if len(text) < 6 and should_filter_by_dots(text):
        return ""
    return text


def sanitize_and_normalize_extracted_text(input_text: str, remove_inner_quotes=False) -> str:
    safe_input_text = sanitize_text_for_encoding(input_text)
    if safe_input_text:
        return normalize_extracted_info(safe_input_text, remove_inner_quotes=remove_inner_quotes)
    return ""


def normalize_entity_name(input_text: str) -> str:
    return sanitize_and_normalize_extracted_text(input_text, remove_inner_quotes=True)


def split_string_by_multi_markers(content: str, markers: list) -> list:
    if not markers:
        return [content]
    results = re.split("|".join(re.escape(marker) for marker in markers), content)
    return [r.strip() for r in results if r.strip()]


# ------------------------------------------------------------- operate.py


def _truncate_entity_identifier(name: str, max_length: int, chunk_key: str, kind: str) -> str:
    if len(name) > max_length:
        return name[:max_length]
    return name


def _normalize_and_validate_entity_type(entity_type: str, context: str):
    if not entity_type.strip() or any(
        char in entity_type for char in ["'", "(", ")", "<", ">", "|", "/", "\\"]
    ):
        return None
    if "," in entity_type:
        tokens = [t.strip() for t in entity_type.split(",")]
        non_empty = [t for t in tokens if t]
        if not non_empty:
            return None
        entity_type = non_empty[0]
    entity_type = entity_type.replace(" ", "").lower()
    if entity_type in _RESERVED_ENTITY_TYPES:
        return None
    return entity_type


def _handle_single_entity_extraction(record_attributes, chunk_key, timestamp, file_path):
    if len(record_attributes) != 4 or "entity" not in record_attributes[0]:
        return None
    entity_name = normalize_entity_name(record_attributes[1])
    if not entity_name:
        return None
    entity_type = _normalize_and_validate_entity_type(
        sanitize_and_normalize_extracted_text(record_attributes[2], remove_inner_quotes=True),
        f"entity {entity_name}",
    )
    if entity_type is None:
        return None
    return {
        "entity_name": entity_name,
        "entity_type": entity_type,
        "description": sanitize_and_normalize_extracted_text(record_attributes[3]),
        "source_id": chunk_key,
        "file_path": file_path,
        "timestamp": timestamp,
    }


def _handle_single_relationship_extraction(record_attributes, chunk_key, timestamp, file_path):
    if len(record_attributes) != 5 or "relation" not in record_attributes[0]:
        return None
    source = normalize_entity_name(record_attributes[1])
    target = normalize_entity_name(record_attributes[2])
    if not source or not target or source == target:
        return None
    edge_keywords = sanitize_and_normalize_extracted_text(
        record_attributes[3], remove_inner_quotes=True
    )
    edge_keywords = edge_keywords.replace("，", ",")
    edge_description = sanitize_and_normalize_extracted_text(record_attributes[4])
    if not edge_description.strip():
        return None
    return {
        "src_id": source,
        "tgt_id": target,
        "weight": 1.0,
        "description": edge_description,
        "keywords": edge_keywords,
        "source_id": chunk_key,
        "file_path": file_path,
        "timestamp": timestamp,
    }


def _normalize_text_extraction_record_attributes(record_attributes, chunk_key):
    if len(record_attributes) != 5:
        return record_attributes
    prefix = record_attributes[0].strip().lower()
    if "entity" not in prefix or "relation" in prefix:
        return record_attributes
    normalized = list(record_attributes)
    normalized[0] = "relation"
    return normalized


def parse(result, chunk_key, timestamp=0, file_path="unknown_source", tuple_delimiter=TUPLE_DELIMITER,
          completion_delimiter=COMPLETION_DELIMITER):
    maybe_nodes = defaultdict(list)
    maybe_edges = defaultdict(list)
    records = split_string_by_multi_markers(
        result, ["\n", completion_delimiter, completion_delimiter.lower()]
    )
    for record in records:
        record = record.strip()
        entity_records = split_string_by_multi_markers(
            record, [f"{tuple_delimiter}entity{tuple_delimiter}"]
        )
        for entity_record in entity_records:
            if not entity_record.startswith("entity") and not entity_record.startswith("relation"):
                entity_record = f"entity{tuple_delimiter}{entity_record}"
            entity_relation_records = split_string_by_multi_markers(
                entity_record,
                [
                    f"{tuple_delimiter}relationship{tuple_delimiter}",
                    f"{tuple_delimiter}relation{tuple_delimiter}",
                ],
            )
            for entity_relation_record in entity_relation_records:
                if not entity_relation_record.startswith("entity") and not entity_relation_record.startswith("relation"):
                    entity_relation_record = f"relation{tuple_delimiter}{entity_relation_record}"
                record_attributes = split_string_by_multi_markers(
                    entity_relation_record, [tuple_delimiter]
                )
                record_attributes = _normalize_text_extraction_record_attributes(
                    record_attributes, chunk_key
                )
                entity_data = _handle_single_entity_extraction(
                    record_attributes, chunk_key, timestamp, file_path
                )
                if entity_data is not None:
                    maybe_nodes[entity_data["entity_name"]].append(entity_data)
                    continue
                relationship_data = _handle_single_relationship_extraction(
                    record_attributes, chunk_key, timestamp, file_path
                )
                if relationship_data is not None:
                    key = tuple(sorted((relationship_data["src_id"], relationship_data["tgt_id"])))
                    maybe_edges[key].append(relationship_data)
    return {
        "nodes": [[name, rows] for name, rows in maybe_nodes.items()],
        "edges": [[list(key), rows] for key, rows in maybe_edges.items()],
    }


CASES = [
    {
        "name": "reference_example_quoted",
        "chunk_key": "chunk-aaa",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"Alex"<|#|>"person"<|#|>"Alex is a character."\n'
            'relation<|#|>"Alex"<|#|>"Taylor"<|#|>"observes"<|#|>"Alex observes Taylor."\n'
            'entity<|#|>"The Device"<|#|>"technology"<|#|>"Central to the story."<|COMPLETE|>'
        ),
    },
    {
        "name": "unquoted_fields_and_spacing",
        "chunk_key": "chunk-bbb",
        "file_path": "docs/a.md",
        "timestamp": 0,
        "raw": (
            'entity<|#|>  acme corp <|#|> organization <|#|> makes things\n'
            'relation<|#|> acme corp <|#|> beta labs <|#|> owns <|#|> Acme owns Beta Labs'
        ),
    },
    {
        "name": "malformed_and_multiline",
        "chunk_key": "chunk-ccc",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"A"<|#|>"B"\n'
            'entity<|#|>"  "<|#|>"B"<|#|>"d"\n'
            'entity<|#|>"C"<|#|>"D"<|#|>"first line\nsecond line"\n'
            'relation<|#|>"C"<|#|>"D"<|#|>"relates"<|#|>'
        ),
    },
    {
        "name": "mis_prefixed_relation_row",
        "chunk_key": "chunk-fff",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": 'entity<|#|>"ACME"<|#|>"BETA"<|#|>"owns"<|#|>"Acme owns Beta."',
    },
    {
        "name": "merged_rows_one_line",
        "chunk_key": "chunk-ggg",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"ACME"<|#|>"organization"<|#|>"A company."<|#|>'
            'entity<|#|>"BETA"<|#|>"organization"<|#|>"Another."'
        ),
    },
    {
        "name": "escaped_entities_and_controls",
        "chunk_key": "chunk-eee",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": 'entity<|#|>"A&amp;B\x07"<|#|>"org"<|#|>"R&amp;D unit"',
    },
    {
        "name": "full_width_and_cjk_spacing",
        "chunk_key": "chunk-hhh",
        "file_path": "docs/zh.md",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"ＡＣＭＥ"<|#|>"组织"<|#|>"中文 描述 测试"\n'
            'relation<|#|>"ＡＣＭＥ"<|#|>"ＢＥＴＡ"<|#|>"拥有"<|#|>"中文 描述 之间 的 关系"'
        ),
    },
    {
        "name": "quoted_names_and_keywords",
        "chunk_key": "chunk-iii",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'entity<|#|>""Alex""<|#|>""person""<|#|>""Quoted name.""\n'
            'relation<|#|>""Alex""<|#|>""Taylor""<|#|>""observes, watches""<|#|>""Watches closely.""'
        ),
    },
    {
        "name": "self_relation_and_empty_description",
        "chunk_key": "chunk-jjj",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'relation<|#|>"ACME"<|#|>"ACME"<|#|>"self"<|#|>"Self relation."\n'
            'relation<|#|>"ACME"<|#|>"BETA"<|#|>"owns"<|#|>"   "'
        ),
    },
    {
        "name": "numeric_only_names",
        "chunk_key": "chunk-kkk",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"12"<|#|>"data"<|#|>"A short number."\n'
            'entity<|#|>"1.2.3"<|#|>"data"<|#|>"Dotted number."\n'
            'entity<|#|>"1234"<|#|>"data"<|#|>"Long enough number."'
        ),
    },
    {
        "name": "book_title_and_chinese_quotes",
        "chunk_key": "chunk-lll",
        "file_path": "docs/zh.md",
        "timestamp": 0,
        "raw": (
            'entity<|#|>"《本书》"<|#|>"content"<|#|>"书名。"\n'
            'entity<|#|>"“引用”"<|#|>"content"<|#|>"引用的内容。"'
        ),
    },
    {
        "name": "relationship_interchangeable_prefix",
        "chunk_key": "chunk-mmm",
        "file_path": "unknown_source",
        "timestamp": 0,
        "raw": 'relationship<|#|>"ACME"<|#|>"BETA"<|#|>"owns"<|#|>"Uses the long form."',
    },
]


def main():
    payload = []
    for case in CASES:
        expected = parse(
            case["raw"],
            case["chunk_key"],
            case.get("timestamp", 0),
            case.get("file_path", "unknown_source"),
        )
        payload.append(
            {
                "name": case["name"],
                "chunk_key": case["chunk_key"],
                "file_path": case.get("file_path", "unknown_source"),
                "timestamp": case.get("timestamp", 0),
                "raw": case["raw"],
                "expected": expected,
            }
        )
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
