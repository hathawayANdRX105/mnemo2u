#!/usr/bin/env python3
"""Extract prompt templates from the reference implementation into plain files.

Loads `refs/nano-graphrag/nano_graphrag/prompt.py` standalone (it has no
imports of its own) and writes every prompt we port into
`src/graph/prompts/<key>.txt`, preserving the text byte for byte.

Usage:
    .venv-ref/bin/python tools/golden/gen_prompts.py
"""

import importlib.util
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
REF_PROMPT = ROOT / "todo" / "refs" / "nano-graphrag" / "nano_graphrag" / "prompt.py"
OUT_DIR = ROOT / "src" / "graph" / "prompts"

KEYS = [
    "entity_extraction",
    "entiti_continue_extraction",
    "entiti_if_loop_extraction",
    "summarize_entity_descriptions",
    "community_report",
    "local_rag_response",
    "global_map_rag_points",
    "global_reduce_rag_response",
    "naive_rag_response",
    "fail_response",
]


def load_reference():
    spec = importlib.util.spec_from_file_location("ref_prompt", REF_PROMPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    ref = load_reference()
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    report = {}
    for key in KEYS:
        text = ref.PROMPTS[key]
        placeholders = sorted(set(re.findall(r"\{(\w+)\}", text)))
        doubled_open = text.count("{{")
        single_open = len(re.findall(r"(?<!\{)\{(?!\{)", text))
        report[key] = {
            "chars": len(text),
            "placeholders": placeholders,
            "doubled_open": doubled_open,
            "single_open_braces": single_open,
        }
        (OUT_DIR / f"{key}.txt").write_text(text, encoding="utf-8")

    # Small constants used to fill the templates.
    constants = {
        "DEFAULT_ENTITY_TYPES": ref.PROMPTS["DEFAULT_ENTITY_TYPES"],
        "DEFAULT_TUPLE_DELIMITER": ref.PROMPTS["DEFAULT_TUPLE_DELIMITER"],
        "DEFAULT_RECORD_DELIMITER": ref.PROMPTS["DEFAULT_RECORD_DELIMITER"],
        "DEFAULT_COMPLETION_DELIMITER": ref.PROMPTS["DEFAULT_COMPLETION_DELIMITER"],
        "default_text_separator": ref.PROMPTS["default_text_separator"],
        "graph_field_sep": ref.GRAPH_FIELD_SEP,
    }
    (OUT_DIR / "constants.json").write_text(
        json.dumps(constants, ensure_ascii=False, indent=1) + "\n", encoding="utf-8"
    )

    json.dump(report, sys.stderr, ensure_ascii=False, indent=1)
    sys.stderr.write("\n")


if __name__ == "__main__":
    main()
