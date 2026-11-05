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
#   scripts/aeneas-spike.sh spike      build, then translate each spike crate with
#                                      Charon and Aeneas, and build the Lean output
#
# The pins live in porting/aeneas/pins.env. They are recorded from a discover
# run and checked on every run; a pin left empty is reported, not enforced
# (trust on first use, as for the elan installer in proofs.yml).
#
# The spike has two crates, so the f32 result cannot hide the loop result:
#   porting/aeneas/spike       sum_u32, a loop over a slice. Must translate and
#                              build; this is the spike's pass condition.
#   porting/aeneas/spike-f32   dot_f32. Its outcome is the f32 verdict and is
#                              reported, not required to pass.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

mode="${1:-}"
case "$mode" in
  discover | build | spike) ;;
  *)
    echo "usage: scripts/aeneas-spike.sh discover|build|spike" >&2
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

# build and spike: the pins decide what is built.
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
# opam reads the dependencies from the opam file in src/. It needs the path
# (./src), not the bare name, which it takes for a package called src.
if ! ls "$work"/aeneas/src/*.opam >/dev/null 2>&1; then
  echo "no opam file in aeneas/src; its dune-project says:" >&2
  head -n 60 "$work/aeneas/src/dune-project" >&2 || true
  die "cannot read the OCaml dependencies of Aeneas"
fi
(cd "$work/aeneas" && opam install ./src --deps-only --with-test --yes)

# Charon's OCaml library (charon-ml) needs the opam dependencies in Charon's own
# opam file, which Aeneas's setup does not install (ppx_deriving_hash, in the
# first attempt). They are installed from the Charon fetched above, at the same
# commit as the Charon that setup builds.
(cd "$work/charon" && opam install . --deps-only --with-test --yes)

# The Aeneas Makefile fetches and builds the Charon at charon-pin, then builds
# Aeneas itself, as its README describes.
(cd "$work/aeneas" && make setup-charon)
(cd "$work/aeneas" && make)

echo "== built"
ls -l "$work/aeneas/bin"
"$work/aeneas/bin/aeneas" -version || true

if [ "$mode" = "build" ]; then
  exit 0
fi

# spike: translate each crate with Charon (to LLBC), then with Aeneas (to Lean).
charon="$work/aeneas/charon/bin/charon"
aeneas="$work/aeneas/bin/aeneas"
nightly="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$work/charon/rust-toolchain")"
[ -n "$nightly" ] || die "no channel in charon/rust-toolchain"
echo "== charon's toolchain: $nightly"
rustup toolchain install "$nightly" --profile minimal -c rustc-dev -c llvm-tools -c rust-src

# Translates one crate. Returns non-zero when Charon or Aeneas fails, and says
# where, so the caller can record the outcome.
translate() { # name crate-dir
  local name="$1" crate="$ROOT/$2" out="$work/out-$1"
  rm -rf "$out"
  mkdir -p "$out"
  cp -r "$crate" "$out/crate"
  echo "== charon: $name ($2)"
  echo "disk before charon: $(df -h / | tail -n 1)"
  (cd "$out/crate" && RUSTUP_TOOLCHAIN="$nightly" CARGO_TARGET_DIR="$out/target" \
    "$charon" cargo --preset=aeneas) || { echo "charon failed on $name"; return 1; }
  ls -l "$out/crate"/*.llbc || { echo "charon wrote no .llbc for $name"; return 1; }

  echo "== aeneas: $name"
  "$aeneas" -help 2>&1 | head -n 80 || true
  "$aeneas" -backend lean -dest "$out/lean" "$out/crate"/*.llbc || { echo "aeneas failed on $name"; return 1; }
  echo "--- generated Lean for $name"
  find "$out/lean" -name '*.lean' | sort
  return 0
}

# Builds the generated Lean for one crate, against the Aeneas Lean library at
# the pinned Aeneas, with that library's own Lean toolchain.
lean_build() { # name
  local name="$1" out="$work/out-$1" proj modules root
  proj="$out/lean-proj"
  mkdir -p "$proj"
  cp -r "$out/lean"/. "$proj/"
  cp "$work/aeneas/backends/lean/lean-toolchain" "$proj/lean-toolchain"
  # Module names are the paths of the generated files, relative to the root.
  modules="$(cd "$proj" && find . -name '*.lean' | sed 's#^\./##; s#\.lean$##; s#/#.#g' | sort | sed 's/.*/"&"/' | paste -sd, -)"
  cat > "$proj/lakefile.toml" <<EOF
name = "spike_$name"
defaultTargets = ["Spike"]

[[require]]
name = "aeneas"
path = "$work/aeneas/backends/lean"

[[lean_lib]]
name = "Spike"
srcDir = "."
roots = [$modules]
EOF
  echo "== lake build: $name (modules: $modules)"
  echo "disk before lake build: $(df -h / | tail -n 1)"
  (cd "$proj" && lake build)
}

loop_status=0
translate sum_u32 porting/aeneas/spike || loop_status=$?
if [ "$loop_status" -eq 0 ]; then
  lean_build sum_u32 || loop_status=$?
fi

f32_status=0
translate dot_f32 porting/aeneas/spike-f32 || f32_status=$?
if [ "$f32_status" -eq 0 ]; then
  lean_build dot_f32 || f32_status=$?
fi

echo "== spike result"
echo "loop (sum_u32): $([ "$loop_status" -eq 0 ] && echo translated and built || echo "FAILED (status $loop_status)")"
echo "f32 (dot_f32): $([ "$f32_status" -eq 0 ] && echo translated and built || echo "not translated or not built (status $f32_status); see the log above")"

[ "$loop_status" -eq 0 ] || die "the loop crate did not translate and build"
