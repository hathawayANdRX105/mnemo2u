#!/usr/bin/env python3
"""Extract prompt templates from the LightRAG reference into plain files.

Loads `refs/LightRAG/lightrag/prompt.py` standalone (its only import is the
logger, stubbed below) and writes every prompt we port into
`src/graph/prompts/<key>.txt`, preserving the text byte for byte. The text
keeps the reference's `{placeholder}` syntax and doubled braces; fill them
with `crate::core::text::fill_template`.

Usage:
    python3 scripts/gen_prompts.py
"""

import ast
import json
import os
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
REF_PROMPT = pathlib.Path(os.environ.get("LIGHTRAG_REF", "/home/hathaway/projects/mnemo2u/todo/refs/LightRAG/lightrag/prompt.py"))
OUT_DIR = ROOT / "src" / "graph" / "prompts"

KEYS = [
    "entity_extraction_system_prompt",
    "entity_extraction_user_prompt",
    "entity_continue_extraction_user_prompt",
    "summarize_entity_descriptions",
    "fail_response",
    "rag_response",
    "keywords_extraction",
    "kg_query_context",
    "naive_query_context",
]

FILE_NAMES = {
    "entity_extraction_system_prompt": "entity_extraction_system.txt",
    "entity_extraction_user_prompt": "entity_extraction_user.txt",
    "entity_continue_extraction_user_prompt": "entity_continue_extraction.txt",
    "summarize_entity_descriptions": "summarize_entity_descriptions.txt",
    "fail_response": "fail_response.txt",
    "rag_response": "rag_response.txt",
    "keywords_extraction": "keywords_extraction.txt",
    "kg_query_context": "kg_query_context.txt",
    "naive_query_context": "naive_query_context.txt",
}

CONSTANT_KEYS = {
    "tuple_delimiter": "DEFAULT_TUPLE_DELIMITER",
    "completion_delimiter": "DEFAULT_COMPLETION_DELIMITER",
    "entity_types_guidance": "default_entity_types_guidance",
}


def load_reference():
    """Read the reference PROMPTS dict without importing the module.

    The reference imports the whole package (`from lightrag.utils import
    logger`), so the file is parsed and only the `PROMPTS[...] = ...` item
    assignments are evaluated with `ast.literal_eval` — no execution of
    reference code.
    """
    tree = ast.parse(REF_PROMPT.read_text(encoding="utf-8"), filename=str(REF_PROMPT))
    prompts = {}
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        target = node.targets[0]
        is_prompts = (
            isinstance(target, ast.Subscript)
            and isinstance(target.value, ast.Name)
            and target.value.id == "PROMPTS"
        )
        if not is_prompts:
            continue
        key = ast.literal_eval(target.slice)
        prompts[key] = ast.literal_eval(node.value)
    if not prompts:
        raise SystemExit(f"no PROMPTS entries in {REF_PROMPT}")
    return prompts


def main():
    prompts = load_reference()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    report = {}
    for key in KEYS:
        text = prompts[key]
        placeholders = sorted(set(re.findall(r"\{(\w+)\}", text)))
        path = OUT_DIR / FILE_NAMES[key]
        path.write_text(text, encoding="utf-8")
        report[key] = {"file": FILE_NAMES[key], "placeholders": placeholders}

    constants = {
        name: prompts[ref_key] for name, ref_key in CONSTANT_KEYS.items()
    }
    (OUT_DIR / "constants.json").write_text(
        json.dumps(constants, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )
    json.dump(report, sys.stderr, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
