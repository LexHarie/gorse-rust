#!/usr/bin/env python3
"""Writes the exact golden overrides of the top-k filter under D-329 (D-330).

Reads crates/gorse-common/tests/data/heap_golden.json, which Go wrote for `TopKFilter`
at add21b5, and recomputes each top-k case under the D-329 rule, in Python and not with the
Rust code under test: a NaN weight is dropped, and the k first elements in weight-descending,
then value-ascending, order are kept.

A case whose D-329 result differs from Go's recorded result is written to
crates/gorse-common/tests/data/heap_topk_d329_overrides.json, with its index, its name and
its exact expected result. Each override names its causes:

* "nan": the case pushes a NaN weight. Go keeps NaN (`filter.go:35-40` has no check), so its
  output differs (A5).
* "tie": the case pushes equal weights, and Go's choice or order among them differs from
  the D-329 tie-break (D-329).

The script stops if a case differs with neither cause, or if a tie-only case changes its
weights rather than only the order or choice among equal weights. Run from the repository
root. It prints how many cases agree with Go and how many it overrides.
"""

import json
import math
import struct
import sys

GOLDEN = "crates/gorse-common/tests/data/heap_golden.json"
OUT = "crates/gorse-common/tests/data/heap_topk_d329_overrides.json"


def weight_of(kind, hex_bits):
    fmt = ">f" if kind == "int32/float32" else ">d"
    return struct.unpack(fmt, bytes.fromhex(hex_bits))[0]


def value_of(kind, raw):
    return int(raw) if kind == "int32/float32" else str(raw)


def golden_result(kind, method, result):
    if method == "pop_all":
        return [(value_of(kind, v), w.lower()) for v, w in result]
    return [value_of(kind, v) for v in result]


def d329(kind, pushes, k, method):
    live = [
        (value_of(kind, v), weight_of(kind, w), w.lower())
        for v, w in pushes
        if not math.isnan(weight_of(kind, w))
    ]
    kept = sorted(live, key=lambda t: (-t[1], t[0]))[: max(k, 0)]
    if method == "pop_all":
        return [(v, w_hex) for v, _, w_hex in kept]
    return [v for v, _, _ in kept]


def causes(kind, pushes):
    weights = [weight_of(kind, w) for _, w in pushes]
    live = [x for x in weights if not math.isnan(x)]
    found = []
    if len(live) != len(weights):
        found.append("nan")
    if len(set(live)) != len(live):
        found.append("tie")
    return found


def main():
    golden = json.load(open(GOLDEN))
    overrides = []
    agree = 0
    for index, case in enumerate(golden["top_k"]):
        kind, method, k, pushes = case["kind"], case["method"], case["k"], case["pushes"]
        expected = d329(kind, pushes, k, method)
        go = golden_result(kind, method, case["result"])
        if expected == go:
            agree += 1
            continue
        found = causes(kind, pushes)
        if not found:
            sys.exit(f"case {index} {case['name']}: differs with no NaN and no tie")
        if found == ["tie"] and method == "pop_all":
            # A tie may change which equal weight is kept, or the order among them,
            # but never the weights themselves.
            if sorted(w for _, w in go) != sorted(w for _, w in expected):
                sys.exit(f"case {index} {case['name']}: tie-only case changes the weights")
        if method == "pop_all":
            result = [[v, w] for v, w in expected]
        else:
            result = list(expected)
        overrides.append(
            {"index": index, "name": case["name"], "causes": found, "result": result}
        )
    out = {
        "decision": "D-330",
        "rule": "D-329: weight descending, then value ascending; NaN rejected (A5)",
        "source": "heap_golden.json, the Go golden at add21b5",
        "cases": overrides,
    }
    with open(OUT, "w") as f:
        json.dump(out, f, indent=1)
        f.write("\n")
    print(f"agree with Go: {agree}; overridden: {len(overrides)}")


if __name__ == "__main__":
    main()
