#!/usr/bin/env bash
# Profiles nanoda on the exports of G2 (D-372). It applies scripts/nanoda-profile.patch to
# the pinned nanoda, which logs every declaration that takes at least a second, and a
# heartbeat each minute, to stderr. The run is stopped after MINUTES. The log, and a copy
# with the declaration names filled in, are written next to the exports. The script exits
# 0 when the check ends or the budget runs out, so the log can be kept as an artifact.
#
# Usage: scripts/profile-nanoda.sh DIR MINUTES
#   DIR holds exports.txt and syntax.txt, as written by scripts/check-kernels.sh export.
set -euo pipefail

cd "$(dirname "$0")/.."
. proofs/tools.env

dir="$1"
minutes="$2"
[ -s "$dir/exports.txt" ] || { echo "profile: no exports.txt in $dir" >&2; exit 1; }

work="$(mktemp -d)"
trap "rm -rf '$work'" EXIT

fetch_rev() {
  local repo="$1" rev="$2" dest="$3" head
  mkdir -p "$dest"
  git -C "$dest" init -q
  git -C "$dest" fetch -q --depth 1 "$repo" "$rev"
  git -C "$dest" checkout -q FETCH_HEAD
  head="$(git -C "$dest" rev-parse HEAD)"
  if [ "$head" != "$rev" ]; then
    echo "profile: $repo resolved to $head, not the pinned $rev" >&2
    exit 1
  fi
}

fetch_rev "$NANODA_REPO" "$NANODA_REV" "$work/nanoda"
patch_file="$PWD/scripts/nanoda-profile.patch"
(cd "$work/nanoda" && git apply "$patch_file")
(cd "$work/nanoda" && cargo build --release --locked --bin nanoda_bin 2>&1 | tail -1)
nanoda="$work/nanoda/target/release/nanoda_bin"

cat > "$work/profile.json" <<JSON
{
  "export_file_path": "$dir/exports.txt",
  "use_stdin": false,
  "permitted_axioms": ["propext", "Classical.choice", "Quot.sound"],
  "unpermitted_axiom_hard_error": true,
  "nat_extension": true,
  "string_extension": true,
  "print_success_message": true
}
JSON

start=$(date +%s)
set +e
timeout "${minutes}m" "$nanoda" "$work/profile.json" > "$dir/profile.log" 2>&1
status=$?
set -e
elapsed=$(( $(date +%s) - start ))
echo "profile: nanoda exit status $status after ${elapsed}s (124 means the budget ran out)"
python3 scripts/annotate-nanoda-log.py "$dir/exports.txt" "$dir/profile.log" || true
echo "== last heartbeats"
grep "DIAG heartbeat" "$dir/profile.log.named" | tail -5 || true
echo "== result lines"
grep -E "Checked|panicked" "$dir/profile.log.named" | head -3 || true
exit 0
