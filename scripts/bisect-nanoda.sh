#!/usr/bin/env bash
# Scratch diagnostic (not for merge): runs nanoda on the export of each named
# theorem alone, to find which declaration nanoda v0.2.0 cannot read.
set -uo pipefail

cd "$(dirname "$0")/.."
. proofs/tools.env

work="$(mktemp -d)"

fetch_rev() {
  local repo="$1" rev="$2" dest="$3" head
  mkdir -p "$dest"
  git -C "$dest" init -q
  git -C "$dest" fetch -q --depth 1 "$repo" "$rev"
  git -C "$dest" checkout -q FETCH_HEAD
  head="$(git -C "$dest" rev-parse HEAD)"
  [ "$head" = "$rev" ] || { echo "resolved to $head, not $rev"; exit 1; }
}

fetch_rev "$NANODA_REPO" "$NANODA_REV" "$work/nanoda"
(cd "$work/nanoda" && cargo build --release --locked --bin nanoda_bin) || { echo "nanoda build failed"; exit 1; }
nanoda="$work/nanoda/target/release/nanoda_bin"

cp -R proofs/kernel/lean4export "$work/lean4export"
(cd "$work/lean4export" && lake build lean4export) || { echo "lean4export build failed"; exit 1; }
exporter="$work/lean4export/.lake/build/bin/lean4export"

names=(
  Gorse.scaffold_true
  Gorse.Spec.Metrics.hits_le_card
  Gorse.Spec.Metrics.precision_le_one
  Gorse.Spec.Metrics.recall_le_one
  Gorse.Spec.Metrics.ndcg_gt_one_of_dup
  Real.log_one
)

summary=()
for name in "${names[@]}"; do
  export_file="$work/$name.export.txt"
  (cd proofs && lake env "$exporter" Gorse -- "$name") > "$export_file" 2> "$work/$name.export.err"
  ecode=$?
  lines=$(wc -l < "$export_file")
  cat > "$work/$name.json" <<JSON
{
  "export_file_path": "$export_file",
  "use_stdin": false,
  "permitted_axioms": ["propext", "Classical.choice", "Quot.sound"],
  "unpermitted_axiom_hard_error": true,
  "nat_extension": true,
  "string_extension": true,
  "print_success_message": true
}
JSON
  out="$("$nanoda" "$work/$name.json" 2>&1)"
  ncode=$?
  echo "=== $name: export exit $ecode, $lines lines; nanoda exit $ncode"
  echo "$out" | grep -E 'panicked|error|Error|no errors' | head -5
  summary+=("$name export=$ecode lines=$lines nanoda=$ncode")
done

echo "=== summary"
printf '%s\n' "${summary[@]}"
exit 0
