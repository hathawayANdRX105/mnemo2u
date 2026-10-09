#!/usr/bin/env python3
"""Generate chunking golden fixtures from the reference algorithm.

The two functions below are copied verbatim from
`refs/nano-graphrag/nano_graphrag/_op.py` (chunking_by_token_size :31-58,
get_chunks :94-108); only the imports are reduced to `tiktoken` so the script
runs without the reference package's heavy dependencies.

Usage:
    .venv-ref/bin/python tools/golden/gen_chunks.py > tests/fixtures/chunk_golden.json
"""

import hashlib
import json
import sys

import tiktoken

TOK = tiktoken.encoding_for_model("gpt-4o")

OVERLAP = 100
MAX_TOKENS = 1200


def chunking_by_token_size(tokens_list, doc_keys, overlap_token_size=OVERLAP, max_token_size=MAX_TOKENS):
    results = []
    for index, tokens in enumerate(tokens_list):
        chunk_token = []
        lengths = []
        for start in range(0, len(tokens), max_token_size - overlap_token_size):
            chunk_token.append(tokens[start : start + max_token_size])
            lengths.append(min(max_token_size, len(tokens) - start))

        chunk_texts = [TOK.decode(c) for c in chunk_token]

        for i, chunk in enumerate(chunk_texts):
            results.append(
                {
                    "tokens": lengths[i],
                    "content": chunk.strip(),
                    "chunk_order_index": i,
                    "full_doc_id": doc_keys[index],
                }
            )
    return results


def get_chunks(new_docs, overlap_token_size=OVERLAP, max_token_size=MAX_TOKENS):
    inserting_chunks = {}
    new_docs_list = list(new_docs.items())
    docs = [new_doc[1]["content"] for new_doc in new_docs_list]
    doc_keys = [new_doc[0] for new_doc in new_docs_list]

    tokens = [TOK.encode(doc) for doc in docs]
    chunks = chunking_by_token_size(
        tokens, doc_keys=doc_keys, overlap_token_size=overlap_token_size, max_token_size=max_token_size
    )
    for chunk in chunks:
        inserting_chunks.update(
            {f"chunk-{hashlib.md5(chunk['content'].encode()).hexdigest()}": chunk}
        )
    return inserting_chunks


def main():
    long_paragraph = (
        "The three-store design keeps the truth in one transactional database, "
        "derives the graph and the vector index from it, and rebuilds either "
        "derivative at will. "
    )
    docs = {
        "doc-short": {"content": "ACME Corporation acquired Beta Labs in 2024."},
        "doc-en": {"content": " ".join(f"Sentence {i} about graph retrieval and caching." for i in range(400))},
        "doc-zh": {"content": "图检索系统需要把文档切块。" * 300},
        "doc-mixed": {"content": (long_paragraph * 60) + "\n\n" + ("混合文本 混排 with english words. " * 120)},
        "doc-whitespace": {"content": "   \n\t  "},
    }
    chunks = get_chunks(docs)
    payload = {
        "config": {"overlap_token_size": OVERLAP, "max_token_size": MAX_TOKENS, "encoding": "o200k_base"},
        "docs": docs,
        "chunks": chunks,
    }
    json.dump(payload, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
