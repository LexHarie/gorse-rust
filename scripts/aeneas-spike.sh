#!/usr/bin/env bash
# The Aeneas spike of the verification plan (P1-3). It runs in CI only: it needs
# github.com for Charon and Aeneas, opam for OCaml, and the Lean toolchain, and
# this sandbox reaches none of the Lean hosts.
#
#   scripts/aeneas-spike.sh discover   print the current Charon and Aeneas heads,
#                                      their archive digests and the files that
#                                      pin their toolchains
#   scripts/aeneas-spike.sh build      fetch the pinned Aeneas, read the Charon it
#                                      pins (charon-pin), and build both
#
# The pins live in porting/aeneas/pins.env. They are recorded from a discover
# run and checked on every run; a pin left empty is reported, not enforced
# (trust on first use, as for the elan installer in proofs.yml).
set -euo pipefail

cd "$(dirname "$0")/.."

mode="${1:-}"
case "$mode" in
  discover | build) ;;
  *)
    echo "usage: scripts/aeneas-spike.sh discover|build" >&2
    exit 2
    ;;
esac

die() {
  echo "aeneas-spike: $*" >&2
  exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Fetches exactly one commit by its head, not a branch, so the digest printed
# here names what the run used.
fetch_rev() { # dir repo rev
  local dir="$1" repo="$2" rev="$3"
  mkdir -p "$dir"
  git -C "$dir" init -q
  git -C "$dir" remote add origin "$repo"
  git -C "$dir" fetch -q --depth 1 origin "$rev"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
}

tree_digest() { # dir
  git -C "$1" archive --format=tar HEAD | sha256sum | cut -d' ' -f1
}

discover_one() { # name repo
  local name="$1" repo="$2" rev dir f
  rev="$(git ls-remote "$repo" HEAD | cut -f1)"
  echo "== $name: $repo HEAD = $rev"
  dir="$work/$name"
  fetch_rev "$dir" "$repo" "$rev"
  echo "$name archive sha256: $(tree_digest "$dir")"

  echo "--- $name top level"
  ls -A "$dir"
  for f in rust-toolchain rust-toolchain.toml backends/lean/lean-toolchain charon-pin; do
    if [ -f "$dir/$f" ]; then
      echo "--- $name/$f"
      cat "$dir/$f"
    fi
  done
  echo "--- $name opam files"
  ls "$dir"/*.opam 2>/dev/null || echo "none at the top level"
  echo "--- $name README lines about opam, OCaml, make, elan or lake"
  grep -n -i -E "opam|ocaml|make|elan|lake" "$dir/README.md" 2>/dev/null | head -60 || true
}

if [ "$mode" = "discover" ]; then
  discover_one charon "https://github.com/AeneasVerif/charon"
  discover_one aeneas "https://github.com/AeneasVerif/aeneas"
  exit 0
fi

# build: the pins decide what is built.
# shellcheck source=../porting/aeneas/pins.env
. porting/aeneas/pins.env

check_pin() { # name dir rev digest
  local name="$1" dir="$2" rev="$3" want="$4" got
  [ "$(git -C "$dir" rev-parse HEAD)" = "$rev" ] || die "$name: checked out a different commit than $rev"
  got="$(tree_digest "$dir")"
  echo "$name $rev archive sha256 $got"
  if [ -z "$want" ]; then
    echo "$name: no archive digest pinned yet; record $got in porting/aeneas/pins.env"
  elif [ "$got" != "$want" ]; then
    die "$name: archive digest $got, pinned $want"
  fi
}

[ -n "$AENEAS_REV" ] || die "AENEAS_REV is not pinned in porting/aeneas/pins.env"

fetch_rev "$work/aeneas" "$AENEAS_REPO" "$AENEAS_REV"
check_pin aeneas "$work/aeneas" "$AENEAS_REV" "$AENEAS_TREE_SHA256"

# charon-pin is a comment line followed by the commit, so take the SHA line.
charon_rev="$(grep -E -x '[0-9a-f]{40}' "$work/aeneas/charon-pin" | tail -n 1 || true)"
echo "charon-pin in aeneas: ${charon_rev:-none}"
[[ "$charon_rev" =~ ^[0-9a-f]{40}$ ]] || die "charon-pin is not a full commit SHA: $charon_rev"
if [ -n "$CHARON_REV" ] && [ "$CHARON_REV" != "$charon_rev" ]; then
  die "CHARON_REV $CHARON_REV differs from the charon-pin of Aeneas $charon_rev"
fi

fetch_rev "$work/charon" "$CHARON_REPO" "$charon_rev"
check_pin charon "$work/charon" "$charon_rev" "$CHARON_TREE_SHA256"

# Aeneas is written in OCaml 5 (its README suggests 5.3.0), with the opam
# dependencies in its src directory.
opam init --bare --yes --disable-sandboxing --no-setup
opam switch create aeneas-spike ocaml-base-compiler.5.3.0 --yes
eval "$(opam env --switch=aeneas-spike --set-switch)"
(cd "$work/aeneas" && opam install src --deps-only --with-test --yes)

# The Aeneas Makefile fetches and builds the Charon at charon-pin, then builds
# Aeneas itself, as its README describes.
(cd "$work/aeneas" && make setup-charon)
(cd "$work/aeneas" && make)

echo "== built"
ls -l "$work/aeneas/bin"
"$work/aeneas/charon/bin/charon" --version || true
"$work/aeneas/bin/aeneas" --help | head -n 5 || true
