#!/usr/bin/env python3
import json
import sys
from collections import Counter, defaultdict
from pathlib import Path


def answer_shape(question: str) -> str:
    q = question.lower()
    if "how many" in q or "how much" in q or "number of" in q:
        return "count"
    if "what brand" in q:
        return "brand"
    if "what breed" in q:
        return "breed"
    if "what color" in q or "what shade" in q:
        return "color"
    if "what was my job" in q or "occupation" in q or "worked as" in q:
        return "occupation"
    if "what certification" in q or "what program" in q or "what course" in q:
        return "program_topic"
    if "what ratio" in q:
        return "ratio"
    if "where" in q:
        return "location_or_source"
    if "what is the name" in q or "called" in q or "named" in q:
        return "title"
    return "other"


def classify_miss(row: dict) -> str:
    hypothesis = str(row.get("hypothesis", "") or "").lower()
    hits = row.get("top_retrieval_hits", []) or []
    hit_texts = [str(hit.get("content", "") or "").lower() for hit in hits]
    hit_relations = [str(hit.get("relation_kind", "") or "") for hit in hits]

    malformed_tokens = (" does", " some", " connect", "start using it", "goes to attend")
    if any(any(token in text for token in malformed_tokens) for text in hit_texts):
        return "malformed_extraction"

    if not hits:
        return "retrieval_miss"

    if "cannot determine" in hypothesis or "don’t give an exact" in hypothesis or "do not name" in hypothesis:
        if all(not relation for relation in hit_relations):
            return "unsupported_answer_shape"
        return "retrieval_miss"

    if any(hit_texts):
        return "synthesis_miss"

    return "retrieval_miss"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: summarize_longmemeval_misses.py <jsonl>", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    misses = [row for row in rows if not row.get("correct")]

    print(f"rows={len(rows)} correct={sum(1 for row in rows if row.get('correct'))} misses={len(misses)}")
    by_class = Counter()
    by_shape = Counter()
    grouped = defaultdict(list)

    for row in misses:
        miss_class = classify_miss(row)
        shape = answer_shape(str(row.get("question", "")))
        by_class[miss_class] += 1
        by_shape[shape] += 1
        grouped[miss_class].append((row.get("question_id"), row.get("question"), row.get("hypothesis")))

    print("\nmiss_class_counts")
    for name, count in by_class.most_common():
        print(f"  {name}: {count}")

    print("\nanswer_shape_counts")
    for name, count in by_shape.most_common():
        print(f"  {name}: {count}")

    print("\nexamples")
    for miss_class, items in grouped.items():
        print(f"\n[{miss_class}]")
        for question_id, question, hypothesis in items[:5]:
            print(f"  {question_id}: {question}")
            print(f"    hypothesis: {hypothesis}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
