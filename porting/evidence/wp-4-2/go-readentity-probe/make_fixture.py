"""Builds crates/gorse-server/src/rest/tests/go_decode.jsonl from the probe's corpus and oracle.

Run from this directory: python3 make_fixture.py > ../../../../../crates/gorse-server/src/rest/tests/go_decode.jsonl
"""
import json
import sys


def depth(text):
    """The deepest nesting of arrays and objects in a JSON text."""
    level = deepest = 0
    in_string = escaped = False
    for ch in text:
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
        elif ch in "[{":
            level += 1
            deepest = max(deepest, level)
        elif ch in "]}":
            level -= 1
    return deepest


corpus = [json.loads(line) for line in open("corpus4.jsonl")]
oracle = [json.loads(line) for line in open("oracle4.jsonl")]
assert len(corpus) == len(oracle), "the corpus and the oracle have one line per body"

for case, out in zip(corpus, oracle):
    data = bytes.fromhex(case["hex"]) if "hex" in case else case["body"].encode()
    if len(data) > 300:
        continue  # the nesting cases, which the Rust tests build themselves
    if case["ct"] == "application/xml" and b"<" in data:
        continue  # XML is not decoded (X-WP42-17)
    if out["value"] and out["value"].startswith("MARSHAL"):
        continue  # Go decodes it but cannot marshal the value back
    if out["value"] and depth(out["value"]) > 100:
        continue  # the test's JSON reader stops at serde's 128-level limit
    row = {"target": case["target"], "ct": case["ct"]}
    if "hex" in case:
        row["hex"] = case["hex"]
    else:
        row["body"] = case["body"]
    row["err"] = out["err"]
    row["value"] = out["value"]
    sys.stdout.write(json.dumps(row, ensure_ascii=False) + "\n")
