#!/usr/bin/env bash
# The G1 checks of the verification plan, for the Lean project in proofs/.
#
#   1. No proof source names sorry, admit, native_decide, implemented_by or
#      extern, and none declares an axiom.
#   2. lake build is clean on the pinned toolchain and Mathlib.
#   3. #print axioms of every exported theorem lists only propext,
#      Classical.choice and Quot.sound (or no axioms at all).
#
# Usage:
#   scripts/check-proofs.sh              run the checks (needs lake on PATH)
#   scripts/check-proofs.sh --self-test  check that a planted sorry fails step 1
#
# Exits non-zero when a check fails. A check that cannot run also fails.
set -euo pipefail

cd "$(dirname "$0")/.."

FORBIDDEN='(\b(sorry|admit|native_decide|implemented_by|extern)\b|^[[:space:]]*axiom[[:space:]])'
ALLOWED_AXIOMS=" propext Classical.choice Quot.sound "

# Step 1. Prints each forbidden use and returns 1 if there is any.
scan_forbidden() {
  if grep -rnE --include='*.lean' --exclude-dir=.lake "$FORBIDDEN" "$1"; then
    return 1
  fi
  return 0
}

if [ "${1:-}" = "--self-test" ]; then
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
  mkdir -p "$work/Gorse"
  cp proofs/Gorse/Scaffold.lean "$work/Gorse/Scaffold.lean"
  if scan_forbidden "$work" >/dev/null; then
    echo "self-test: the clean scaffold was reported as clean, as expected"
  else
    echo "self-test: the clean scaffold was flagged; the scan is wrong" >&2
    exit 1
  fi
  printf '\ntheorem planted : 1 = 1 := sorry\n' >>"$work/Gorse/Scaffold.lean"
  if scan_forbidden "$work" >/dev/null; then
    echo "self-test: a planted sorry was NOT caught" >&2
    exit 1
  fi
  echo "self-test: a planted sorry fails step 1, as required"
  exit 0
fi

echo "== step 1: forbidden tokens and axioms in the proof sources"
if ! scan_forbidden proofs; then
  echo "G1 step 1 failed: forbidden tokens above" >&2
  exit 1
fi

echo "== step 2: lake build"
(cd proofs && lake build)

echo "== step 3: #print axioms of the exported theorems"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# The exported theorems, from proofs/exports.txt. The G2 export reads the same list.
mapfile -t exports < <(grep -v -E '^(#|$)' proofs/exports.txt)
{
  echo "import Gorse"
  for name in "${exports[@]}"; do
    echo "#print axioms $name"
  done
} >"$work/AxiomsCheck.lean"
(cd proofs && lake env lean "$work/AxiomsCheck.lean") >"$work/axioms.txt"
cat "$work/axioms.txt"

lines=0
for name in "${exports[@]}"; do
  line="$(grep -F "'$name' " "$work/axioms.txt" || true)"
  if [ -z "$line" ]; then
    echo "G1 step 3 failed: no #print axioms output for $name" >&2
    exit 1
  fi
  lines=$((lines + 1))
  if printf '%s' "$line" | grep -q 'does not depend on any axioms'; then
    continue
  fi
  list="$(printf '%s' "$line" | sed -n 's/.*depends on axioms: \[\(.*\)\].*/\1/p')"
  if [ -z "$list" ]; then
    echo "G1 step 3 failed: cannot read the axioms of $name" >&2
    exit 1
  fi
  IFS=', ' read -r -a found <<<"$list"
  for axiom in "${found[@]}"; do
    case "$ALLOWED_AXIOMS" in
      *" $axiom "*) ;;
      *) echo "G1 step 3 failed: $name depends on $axiom" >&2; exit 1 ;;
    esac
  done
done
echo "G1: $lines exported theorems checked; every check passed"
