#!/usr/bin/env bash
# The Aeneas spike of the verification plan (P1-3). It runs in CI only: it needs
# github.com for Charon and Aeneas, opam for OCaml, and the Lean toolchain, and
# this sandbox reaches none of the Lean hosts.
#
#   scripts/aeneas-spike.sh discover   print the current Charon and Aeneas heads,
#                                      their archive digests and the files that
#                                      pin their toolchains
#
# The pins for the run mode are recorded from a discover run, then checked on
# every run (trust on first use, as for the elan installer in proofs.yml).
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "${1:-}" != "discover" ]; then
  echo "usage: scripts/aeneas-spike.sh discover" >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Fetches exactly one commit by its head, not a branch, so the digest printed
# here names what the run used.
discover_one() {
  local name="$1" repo="$2" rev dir f
  rev="$(git ls-remote "$repo" HEAD | cut -f1)"
  echo "== $name: $repo HEAD = $rev"
  dir="$work/$name"
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$repo"
  git -C "$dir" fetch -q --depth 1 origin "$rev"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  echo "$name archive sha256: $(git -C "$dir" archive --format=tar HEAD | sha256sum | cut -d' ' -f1)"

  echo "--- $name top level"
  ls -A "$dir"
  for f in rust-toolchain rust-toolchain.toml backends/lean/lean-toolchain; do
    if [ -f "$dir/$f" ]; then
      echo "--- $name/$f"
      cat "$dir/$f"
    fi
  done
  echo "--- $name opam files"
  ls "$dir"/*.opam 2>/dev/null || echo "none at the top level"
  echo "--- $name README headings"
  grep -n "^#" "$dir/README.md" 2>/dev/null | head -60 || echo "no README.md"
  echo "--- $name README lines about opam, OCaml, make, elan or lake"
  grep -n -i -E "opam|ocaml|make|elan|lake" "$dir/README.md" 2>/dev/null | head -60 || true
  echo "--- $name Makefile, first 80 lines"
  head -80 "$dir/Makefile" 2>/dev/null || echo "no Makefile"
}

discover_one charon "https://github.com/AeneasVerif/charon"
discover_one aeneas "https://github.com/AeneasVerif/aeneas"
