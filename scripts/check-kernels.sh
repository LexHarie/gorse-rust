#!/usr/bin/env bash
# G2, the independent check of the verification plan, for the Lean project in proofs/.
#
#   1. lean4checker replays the declarations of every module under proofs/Gorse on
#      top of the environment its imports provide.
#   2. nanoda, a second kernel written in Rust, checks the export of the theorems
#      listed in proofs/exports.txt. The export comes from the lean4export copy in
#      proofs/kernel/lean4export, built on the project's own toolchain (its
#      provenance and changes are in NOTICE.md there).
#
# Every external tool is fetched at the revision pinned in proofs/tools.env, and a
# checkout that resolves to any other commit fails. proofs/ must be built first
# (lake build), and lake, elan and cargo must be on PATH.
#
# Usage: scripts/check-kernels.sh
set -euo pipefail

cd "$(dirname "$0")/.."
. proofs/tools.env

work="$(mktemp -d)"
trap "rm -rf '$work'" EXIT

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

echo "== G2 leg 1: lean4checker at $LEAN4CHECKER_REV"
fetch_rev "$LEAN4CHECKER_REPO" "$LEAN4CHECKER_REV" "$work/lean4checker"
(cd "$work/lean4checker" && lake build lean4checker)
checker="$work/lean4checker/.lake/build/bin/lean4checker"
[ -x "$checker" ] || { echo "G2: lean4checker was not built" >&2; exit 1; }

modules=(Gorse)
while IFS= read -r f; do
  m="${f#proofs/}"
  m="${m%.lean}"
  modules+=("${m//\//.}")
done < <(find proofs/Gorse -name '*.lean' | LC_ALL=C sort)

for m in "${modules[@]}"; do
  echo "-- lean4checker $m"
  (cd proofs && lake env "$checker" "$m")
done

echo "== G2 leg 2: the export of the exported theorems, checked by nanoda at $NANODA_REV"
cp -R proofs/kernel/lean4export "$work/lean4export"
(cd "$work/lean4export" && lake build lean4export)
exporter="$work/lean4export/.lake/build/bin/lean4export"
[ -x "$exporter" ] || { echo "G2: lean4export was not built" >&2; exit 1; }

mapfile -t exported < <(grep -v -E '^(#|$)' proofs/exports.txt)
(cd proofs && lake env "$exporter" Gorse -- "${exported[@]}") > "$work/export.txt"
echo "export: $(wc -l < "$work/export.txt") lines for ${#exported[@]} theorems"

fetch_rev "$NANODA_REPO" "$NANODA_REV" "$work/nanoda"
(cd "$work/nanoda" && cargo build --release --locked --bin nanoda_bin)
nanoda="$work/nanoda/target/release/nanoda_bin"
[ -x "$nanoda" ] || { echo "G2: nanoda was not built" >&2; exit 1; }

# Only the three semi-official axioms are permitted, and any other axiom in the
# export is a hard error. Nat and String literals are checked by their extensions.
cat > "$work/nanoda.json" <<EOF
{
  "export_file_path": "$work/export.txt",
  "use_stdin": false,
  "permitted_axioms": ["propext", "Classical.choice", "Quot.sound"],
  "unpermitted_axiom_hard_error": true,
  "nat_extension": true,
  "string_extension": true,
  "print_success_message": true
}
EOF
out="$("$nanoda" "$work/nanoda.json")"
echo "$out"
case "$out" in
  *"with no errors"*) ;;
  *) echo "G2: nanoda did not report a clean check" >&2; exit 1 ;;
esac

echo "G2: lean4checker and nanoda both accept every module and exported theorem"
