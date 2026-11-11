#!/usr/bin/env bash
# G2, the independent check of the verification plan, for the Lean project in proofs/.
#
#   1. lean4checker replays the declarations of every module under proofs/Gorse on
#      top of the environment its imports provide.
#   2. nanoda, a second kernel written in Rust, checks the export of every theorem
#      listed in proofs/exports.txt, the Mathlib-dependent ones included (D-321). The
#      export comes from the lean4export copy in proofs/kernel/lean4export, built on
#      the project's own toolchain (its provenance and changes are in NOTICE.md there).
#      Before the project's export, nanoda checks the export of Lean.Syntax from Init,
#      a nested inductive. Without the constructor and recursor dump of NOTICE.md, that
#      export fails, so this check is the regression test of the dump.
#
# Usage:
#   scripts/check-kernels.sh               both legs, for a local run
#   scripts/check-kernels.sh lean4checker  leg 1 only
#   scripts/check-kernels.sh export DIR    writes the two exports of leg 2 to DIR (needs lake)
#   scripts/check-kernels.sh nanoda DIR    checks the exports in DIR with nanoda (leg 2)
#
# CI runs leg 1 in the g1 job, and leg 2 in the g2 job, which exports first, keeps the
# exports as an artifact, and then runs nanoda (D-369). DIR is a path, absolute or from
# the repository root.
#
# Every external tool is fetched at the revision pinned in proofs/tools.env, and a
# checkout that resolves to any other commit fails. proofs/ must be built first
# (lake build), and lake, elan and cargo must be on PATH.
set -euo pipefail

cd "$(dirname "$0")/.."
. proofs/tools.env

work="$(mktemp -d)"
trap "rm -rf '$work'" EXIT

usage() {
  echo "usage: scripts/check-kernels.sh [lean4checker | export DIR | nanoda DIR]" >&2
  exit 2
}

# fetch_rev REPO REV DEST: checks out exactly REV in DEST, or fails.
fetch_rev() {
  local repo="$1" rev="$2" dest="$3" head
  mkdir -p "$dest"
  git -C "$dest" init -q
  git -C "$dest" fetch -q --depth 1 "$repo" "$rev"
  git -C "$dest" checkout -q FETCH_HEAD
  head="$(git -C "$dest" rev-parse HEAD)"
  if [ "$head" != "$rev" ]; then
    echo "G2: $repo resolved to $head, not the pinned $rev" >&2
    exit 1
  fi
}

# Leg 1: lean4checker replays every module of proofs/Gorse.
run_lean4checker() {
  echo "== G2 leg 1: lean4checker at $LEAN4CHECKER_REV"
  fetch_rev "$LEAN4CHECKER_REPO" "$LEAN4CHECKER_REV" "$work/lean4checker"
  (cd "$work/lean4checker" && lake build lean4checker)
  local checker="$work/lean4checker/.lake/build/bin/lean4checker"
  [ -x "$checker" ] || { echo "G2: lean4checker was not built" >&2; exit 1; }

  local modules=(Gorse) f m
  while IFS= read -r f; do
    m="${f#proofs/}"
    m="${m%.lean}"
    modules+=("${m//\//.}")
  done < <(find proofs/Gorse -name '*.lean' | LC_ALL=C sort)

  for m in "${modules[@]}"; do
    echo "-- lean4checker $m"
    (cd proofs && lake env "$checker" "$m")
  done
  echo "G2: lean4checker accepts every module"
}

# The lean4export copy, built on the project's toolchain. Sets $exporter.
build_exporter() {
  cp -R proofs/kernel/lean4export "$work/lean4export"
  (cd "$work/lean4export" && lake build lean4export)
  exporter="$work/lean4export/.lake/build/bin/lean4export"
  [ -x "$exporter" ] || { echo "G2: lean4export was not built" >&2; exit 1; }
}

# Writes DIR/syntax.txt (the self-test) and DIR/exports.txt (every name of exports.txt).
run_export() {
  local dir="$1" exported=()
  mkdir -p "$dir"
  build_exporter
  (cd proofs && lake env "$exporter" Init -- Lean.Syntax) > "$dir/syntax.txt"
  mapfile -t exported < <(grep -v -E '^(#|$)' proofs/exports.txt)
  (cd proofs && lake env "$exporter" Gorse -- "${exported[@]}") > "$dir/exports.txt"
  echo "G2 export: $(wc -l < "$dir/exports.txt") lines for ${#exported[@]} theorems, in $dir"
}

# nanoda_check FILE NAME: nanoda checks the export FILE, and the run must report no errors.
# Only the three semi-official axioms are permitted, and any other axiom in the export is
# a hard error. Nat and String literals are checked by their extensions.
nanoda_check() {
  local file="$1" name="$2" out
  cat > "$work/nanoda-$name.json" <<EOF
{
  "export_file_path": "$file",
  "use_stdin": false,
  "permitted_axioms": ["propext", "Classical.choice", "Quot.sound"],
  "unpermitted_axiom_hard_error": true,
  "nat_extension": true,
  "string_extension": true,
  "print_success_message": true
}
EOF
  out="$("$nanoda" "$work/nanoda-$name.json")"
  echo "$out"
  case "$out" in
    *"with no errors"*) ;;
    *) echo "G2: nanoda did not report a clean check of $name" >&2; exit 1 ;;
  esac
}

# Leg 2: nanoda checks the exports written by run_export into DIR.
run_nanoda() {
  local dir="$1"
  [ -s "$dir/syntax.txt" ] && [ -s "$dir/exports.txt" ] || { echo "G2: no exports in $dir" >&2; exit 1; }
  echo "== G2 leg 2: the exports checked by nanoda at $NANODA_REV"
  fetch_rev "$NANODA_REPO" "$NANODA_REV" "$work/nanoda"
  (cd "$work/nanoda" && cargo build --release --locked --bin nanoda_bin)
  nanoda="$work/nanoda/target/release/nanoda_bin"
  [ -x "$nanoda" ] || { echo "G2: nanoda was not built" >&2; exit 1; }

  echo "-- self-test: the export of Lean.Syntax from Init, a nested inductive (D-321)"
  nanoda_check "$dir/syntax.txt" syntax
  nanoda_check "$dir/exports.txt" exports
  echo "G2: nanoda accepts the self-test and every exported theorem"
}

case "${1:-all}" in
  all)
    [ $# -le 1 ] || usage
    run_lean4checker
    run_export "$work/g2"
    run_nanoda "$work/g2"
    echo "G2: lean4checker and nanoda both accept every module and every exported theorem"
    ;;
  lean4checker)
    [ $# -eq 1 ] || usage
    run_lean4checker
    ;;
  export)
    [ $# -eq 2 ] || usage
    run_export "$2"
    ;;
  nanoda)
    [ $# -eq 2 ] || usage
    run_nanoda "$2"
    ;;
  *)
    usage
    ;;
esac
