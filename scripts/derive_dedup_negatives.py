#!/usr/bin/env python3
"""Derive a Go test file whose candidate lists have no repeats (D-368).

Each line of a Go test file is `(user,item)<TAB>negative<TAB>negative...`. The Go
evaluator ranks `[positive] + negatives` exactly as written, so a repeated negative is
ranked twice. Rust's candidate_list keeps each item once, first seen (D-323, A4). This
script writes the same test file with the repeats removed, so a Go run on the derived
file ranks the candidates that the Rust side ranks (the Lean theorem ndcg_gt_one_of_dup,
D-317).

For each line the script keeps the positive field, keeps the first occurrence of each
negative, drops every negative equal to the positive item, and asserts that the derived
candidate list is exactly the first-seen deduplication of the original one. Nothing else
changes. The training file is not read or written.

Usage:
  derive_dedup_negatives.py INPUT OUTPUT [--manifest FILE] [--label NAME]
  derive_dedup_negatives.py --self-test

The manifest is JSON with the SHA-256 of the input, the output and this script, and the
counts. parity.sh derive-overrides writes one manifest per dataset.
"""

import argparse
import hashlib
import json
import re
import sys
import tempfile
from pathlib import Path

POSITIVE = re.compile(r"^\(([^,()\t]+),([^,()\t]+)\)$")
ZERO_STATS = {"lines": 0, "negatives_in": 0, "negatives_out": 0, "repeats_removed": 0, "positive_removed": 0}


def first_seen(items):
    seen = set()
    out = []
    for item in items:
        if item not in seen:
            seen.add(item)
            out.append(item)
    return out


def check_derivation(lineno, original, derived):
    """The derived candidates are the first-seen deduplication of the original ones."""
    if derived != first_seen(original):
        raise AssertionError(
            f"line {lineno}: the derived candidates are not the first-seen deduplication of the original"
        )
    if set(derived) != set(original):
        raise AssertionError(f"line {lineno}: the derived candidate set differs from the original")


def derive_line(line, lineno):
    """Return (positive field, derived negatives, counts) for one line of a test file."""
    if line == "":
        raise ValueError(f"line {lineno}: an empty line")
    fields = line.split("\t")
    positive_field = fields[0]
    match = POSITIVE.match(positive_field)
    if match is None:
        raise ValueError(f"line {lineno}: the first field is not (user,item): {positive_field!r}")
    negatives = fields[1:]
    if any(neg == "" for neg in negatives):
        raise ValueError(f"line {lineno}: an empty negative field")
    positive_item = match.group(2)
    derived_negatives = [neg for neg in first_seen(negatives) if neg != positive_item]
    check_derivation(lineno, [positive_item] + negatives, [positive_item] + derived_negatives)
    removed = len(negatives) - len(derived_negatives)
    positive_removed = sum(1 for neg in negatives if neg == positive_item)
    counts = {
        "lines": 1,
        "negatives_in": len(negatives),
        "negatives_out": len(derived_negatives),
        "repeats_removed": removed - positive_removed,
        "positive_removed": positive_removed,
    }
    return positive_field, derived_negatives, counts


def derive_text(text):
    """Return (derived text, counts) for the text of a whole test file."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines = lines[:-1]
    out_lines = []
    totals = dict(ZERO_STATS)
    for lineno, line in enumerate(lines, start=1):
        positive_field, derived_negatives, counts = derive_line(line, lineno)
        out_lines.append("\t".join([positive_field] + derived_negatives))
        for key, value in counts.items():
            totals[key] += value
    derived = "".join(line + "\n" for line in out_lines)
    return derived, totals


def sha256_of(data):
    return hashlib.sha256(data).hexdigest()


def derive_file(input_path, output_path, label):
    raw = input_path.read_bytes()
    derived, counts = derive_text(raw.decode("utf-8"))
    output_path.write_bytes(derived.encode("utf-8"))
    return {
        "label": label,
        "script_sha256": sha256_of(Path(__file__).read_bytes()),
        "input": {"sha256": sha256_of(raw), "bytes": len(raw)},
        "output": {"sha256": sha256_of(derived.encode("utf-8")), "bytes": len(derived.encode("utf-8"))},
        "counts": counts,
    }


def self_test():
    cases = [
        # The repeats 5 and 6 are removed; the first occurrence keeps the order.
        ("(1,10)\t5\t6\t5\t7\t6\n", "(1,10)\t5\t6\t7\n", {"lines": 1, "negatives_in": 5, "negatives_out": 3, "repeats_removed": 2, "positive_removed": 0}),
        # A negative equal to the positive item is dropped; the repeat of 21 is removed.
        ("(2,20)\t20\t21\t21\n", "(2,20)\t21\n", {"lines": 1, "negatives_in": 3, "negatives_out": 1, "repeats_removed": 1, "positive_removed": 1}),
        # A line with no negatives is unchanged.
        ("(3,30)\n", "(3,30)\n", {"lines": 1, "negatives_in": 0, "negatives_out": 0, "repeats_removed": 0, "positive_removed": 0}),
        # Two lines; the file keeps its line order and its final newline.
        (
            "(4,40)\t41\t42\t41\t40\t42\t43\n(5,50)\t51\n",
            "(4,40)\t41\t42\t43\n(5,50)\t51\n",
            {"lines": 2, "negatives_in": 7, "negatives_out": 4, "repeats_removed": 2, "positive_removed": 1},
        ),
    ]
    for text, want, want_counts in cases:
        got, counts = derive_text(text)
        assert got == want, f"derive_text({text!r}) gave {got!r}, want {want!r}"
        assert counts == want_counts, f"counts for {text!r} were {counts}, want {want_counts}"

    # Malformed input is refused rather than guessed at: a bad positive field, an empty
    # negative, or an empty line in the middle of the file.
    for bad in ["1,2\t3\n", "(1,2)\t\t3\n", "(1,2)\n\n(3,4)\n"]:
        try:
            derive_text(bad)
        except ValueError:
            pass
        else:
            raise AssertionError(f"derive_text accepted malformed input {bad!r}")

    # The assertion must catch a derivation that drops a distinct negative.
    try:
        check_derivation(1, ["10", "5", "6", "5"], ["10", "5"])
    except AssertionError:
        pass
    else:
        raise AssertionError("check_derivation accepted a dropped distinct negative")

    # The file-level path writes the same bytes as the text path and records digests.
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "test.txt"
        dst = Path(tmp) / "derived.txt"
        src.write_text("(4,40)\t41\t42\t41\t40\t42\t43\n(5,50)\t51\n", encoding="utf-8")
        manifest = derive_file(src, dst, "self-test")
        assert dst.read_text(encoding="utf-8") == "(4,40)\t41\t42\t43\n(5,50)\t51\n"
        assert manifest["output"]["sha256"] == sha256_of(dst.read_bytes())
        assert manifest["input"]["sha256"] == sha256_of(src.read_bytes())
    print("derive_dedup_negatives: self-test passed (4 cases, 3 refusals, 1 assertion check, file path)")


def main(argv):
    if argv[:1] == ["--self-test"]:
        self_test()
        return 0
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--label", default="")
    args = parser.parse_args(argv)
    manifest = derive_file(args.input, args.output, args.label)
    text = json.dumps(manifest, indent=2, sort_keys=True)
    if args.manifest:
        args.manifest.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
