#!/usr/bin/env python3
"""Generates proofs/Gorse/Spec/Log2Table.lean (P4-10, D-354) from the table in the Rust source.

The table LOG2_TABLE of crates/gorse-model/src/cf/evaluator.rs holds the bit patterns of
rn32(log2 k) for k = 2..65. For each entry the script computes, in exact rational arithmetic:

* the significand m and the unit exponent e of the entry, c = m * 2^e and h = 2^e / 2;
* a dyadic y1 with 8 y1 <= 0.6931471803 (c + h), so exp (8 y1) <= exp (log 2 (c + h)); and a
  lower bound L of exp y1, the degree-11 Taylor sum less its error bound (Real.exp_bound);
* a dyadic y2 with 0.6931471808 (c - h) <= 8 y2, and an upper bound U of exp y2;
* the checks L^8 > k and U^8 < k, which decide the two halves of the rounding interval.

The Lean file is the template scripts/gen-log2-table/head.lean, the decodings, and one entry
lemma for each k that is not an exact power of two (k = 2, 4, 16 are proved from the grid).
The script writes the file; `scripts/check-oracle-log2.sh` does not read it. A committed file
that differs from the output of this script is an error (the G9 job runs `--check`).

Usage: gen.py [--check]
"""
import math
import re
import sys
from fractions import Fraction as F
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUST = ROOT / "crates/gorse-model/src/cf/evaluator.rs"
HEAD = ROOT / "scripts/gen-log2-table/head.lean"
OUT = ROOT / "proofs/Gorse/Spec/Log2Table.lean"

LLO = F("0.6931471803")  # Real.log_two_gt_d9
LHI = F("0.6931471808")  # Real.log_two_lt_d9
D = 2**40
FACT12 = math.factorial(12)


def taylor(y):
    return sum(y**i / math.factorial(i) for i in range(12))


def err(y):
    return y**12 * F(13, FACT12 * 12)


def floor_d(x):
    return F(math.floor(x * D), D)


def ceil_d(x):
    return F(math.ceil(x * D), D)


def table_bits():
    text = RUST.read_text()
    block = re.search(r"pub const LOG2_TABLE: \[u32; 64\] = \[(.*?)\];", text, re.S)
    bits = [int(x, 16) for x in re.findall(r"0x([0-9A-Fa-f]{8})", block.group(1))]
    assert len(bits) == 64, len(bits)
    return {k: b for k, b in zip(range(2, 66), bits)}


def lit(q):
    if q.denominator == 1:
        return f"({q.numerator} : ℝ)"
    return f"({q.numerator} / {q.denominator} : ℝ)"


def entry(k, b):
    E = (b >> 23) & 0xFF
    S = b & ((1 << 23) - 1)
    m = (1 << 23) + S
    e = E - 150
    C = F(m) * F(2) ** e
    H = F(2) ** e / 2
    Y1 = floor_d(LLO * (C + H) / 8)
    L = floor_d(taylor(Y1) - err(Y1))
    Y2 = ceil_d(LHI * (C - H) / 8)
    U = ceil_d(taylor(Y2) + err(Y2))
    assert 0 <= Y1 <= 1 and 0 <= Y2 <= 1 and 0 <= L and U <= 2
    assert 8 * Y1 <= LLO * (C + H) and LHI * (C - H) <= 8 * Y2
    assert L**8 > k, k
    assert U**8 < k, k
    return f"""theorem entry_{k} : rn (Real.logb 2 {k}) = decodeBits 0x{b:08X} := by
  rw [decodeBits_{k}]
  have hb := logb_mem_of_bounds (k := {k}) (c := {lit(C)}) (h := {lit(H)})
    (y1 := {lit(Y1)}) (y2 := {lit(Y2)}) (L := {lit(L)}) (U := {lit(U)})
    (by norm_num) (by linarith [Real.log_two_gt_d9]) (by norm_num)
    (le_trans (by norm_num [Finset.sum_range_succ, Nat.factorial])
      (exp_lower {lit(Y1)} (by norm_num) (by norm_num)))
    (by norm_num) (by linarith [Real.log_two_lt_d9]) (by norm_num)
    (le_trans (exp_upper {lit(Y2)} (by norm_num) (by norm_num))
      (by norm_num [Finset.sum_range_succ, Nat.factorial]))
    (by norm_num)
  rw [rn_entry (m := {m}) (e := {e}) (by norm_num) (by norm_num) (by norm_num) (by norm_num)
    (by norm_num) hb.1 hb.2] <;> norm_num
"""


def decode_lemma(k, b):
    E = (b >> 23) & 0xFF
    S = b & ((1 << 23) - 1)
    m = (1 << 23) + S
    e = E - 150
    return (f"theorem decodeBits_{k} : decodeBits 0x{b:08X} = ({m} : ℝ) * (2:ℝ)^({e} : ℤ) := by\n"
            "  norm_num [decodeBits]\n")


def render():
    bits = table_bits()
    parts = [HEAD.read_text().rstrip("\n") + "\n"]
    parts += [decode_lemma(k, b) + "\n" for k, b in bits.items() if k not in (2, 4, 16)]
    parts += [entry(k, b) + "\n" for k, b in bits.items() if k not in (2, 4, 16)]
    parts.append("end Gorse.Spec.Log2Table\n")
    return "\n".join(parts)


def main(argv):
    text = render()
    if "--check" in argv:
        if OUT.read_text() != text:
            print("gen-log2-table: proofs/Gorse/Spec/Log2Table.lean differs from the generated file",
                  file=sys.stderr)
            return 1
        print("gen-log2-table: the committed Lean file is the generated file")
        return 0
    OUT.write_text(text)
    print(f"gen-log2-table: wrote {OUT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
