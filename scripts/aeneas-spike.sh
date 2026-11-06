#!/usr/bin/env bash
# The Aeneas spike of the verification plan (P1-3). It runs in CI only: it needs
# github.com for Charon and Aeneas, opam for OCaml, and the Lean toolchain, and
# this sandbox reaches none of the Lean hosts.
#
#   scripts/aeneas-spike.sh discover   print the current Charon and Aeneas heads,
#                                      their archive digests and the files that
#                                      pin their toolchains
#   scripts/aeneas-spike.sh charon     fetch and check the pinned Aeneas and the
#                                      Charon it pins, then build Charon
#   scripts/aeneas-spike.sh aeneas     build Aeneas, then translate each spike
#                                      crate with Charon and Aeneas
#   scripts/aeneas-spike.sh lean       fetch the Mathlib cache, then build the
#                                      generated Lean against the Aeneas library
#   scripts/aeneas-spike.sh spike      charon, aeneas and lean in turn
#
# CI runs the stages as separate workflow steps, so the step list shows which one
# failed. The stages share one work directory, named by SPIKE_WORK. Without it,
# the script makes a temporary directory and removes it on exit.
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
#   porting/aeneas/spike-topk  the TopKFilter heap loops on an integer weight (D-331).
#                              Its outcome is the D-331 record and is reported, not
#                              required to pass.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

mode="${1:-}"
case "$mode" in
  discover | charon | aeneas | lean | spike) ;;
  *)
    echo "usage: scripts/aeneas-spike.sh discover|charon|aeneas|lean|spike" >&2
    exit 2
    ;;
esac

die() {
  echo "aeneas-spike: $*" >&2
  exit 1
}

if [ -n "${SPIKE_WORK:-}" ]; then
  work="$SPIKE_WORK"
  mkdir -p "$work"
else
  work="$(mktemp -d)"
  trap 'rm -rf "$work"' EXIT
fi

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

# The stages below: the pins decide what is fetched and built.
# shellcheck source=../porting/aeneas/pins.env
. porting/aeneas/pins.env

# Checks a tree that is already fetched: its commit and its archive digest.
check_pin() { # name dir rev digest
  local name="$1" dir="$2" rev="$3" want="$4" got
  [ -d "$dir/.git" ] || die "$name: $dir is not fetched; run the charon stage first"
  [ "$(git -C "$dir" rev-parse HEAD)" = "$rev" ] || die "$name: checked out a different commit than $rev"
  got="$(tree_digest "$dir")"
  echo "$name $rev archive sha256 $got"
  if [ -z "$want" ]; then
    echo "$name: no archive digest pinned yet; record $got in porting/aeneas/pins.env"
  elif [ "$got" != "$want" ]; then
    die "$name: archive digest $got, pinned $want"
  fi
}

# charon-pin is a comment line followed by the commit, so take the SHA line.
charon_pin_rev() {
  local rev
  rev="$(grep -E -x '[0-9a-f]{40}' "$work/aeneas/charon-pin" | tail -n 1 || true)"
  [[ "$rev" =~ ^[0-9a-f]{40}$ ]] || die "charon-pin is not a full commit SHA: ${rev:-none}"
  echo "$rev"
}

# Each CI stage is a new shell, so each one activates the switch itself.
use_switch() {
  eval "$(opam env --switch=aeneas-spike --set-switch)"
}

stage_charon() {
  [ -n "$AENEAS_REV" ] || die "AENEAS_REV is not pinned in porting/aeneas/pins.env"
  [ -d "$work/aeneas/.git" ] || fetch_rev "$work/aeneas" "$AENEAS_REPO" "$AENEAS_REV"
  check_pin aeneas "$work/aeneas" "$AENEAS_REV" "$AENEAS_TREE_SHA256"

  local charon_rev
  charon_rev="$(charon_pin_rev)"
  echo "charon-pin in aeneas: $charon_rev"
  if [ -n "$CHARON_REV" ] && [ "$CHARON_REV" != "$charon_rev" ]; then
    die "CHARON_REV $CHARON_REV differs from the charon-pin of Aeneas $charon_rev"
  fi
  [ -d "$work/charon/.git" ] || fetch_rev "$work/charon" "$CHARON_REPO" "$charon_rev"
  check_pin charon "$work/charon" "$charon_rev" "$CHARON_TREE_SHA256"

  # Aeneas is written in OCaml 5 (its README suggests 5.3.0). Charon's OCaml
  # library needs the opam dependencies in Charon's own opam file, which
  # Aeneas's setup does not install (ppx_deriving_hash, in the first attempt).
  # They come from the Charon fetched above, at the commit setup builds.
  opam init --bare --yes --disable-sandboxing --no-setup
  opam switch create aeneas-spike ocaml-base-compiler.5.3.0 --yes
  use_switch
  (cd "$work/charon" && opam install . --deps-only --with-test --yes)

  # The Aeneas Makefile fetches and builds the Charon at charon-pin.
  (cd "$work/aeneas" && make setup-charon)
  [ -x "$work/aeneas/charon/bin/charon" ] || die "setup-charon did not build charon/bin/charon"
  echo "== charon built"
}

# Translates one crate. Returns non-zero when Charon or Aeneas fails, and says
# where, so the caller can record the outcome. Needs $charon, $aeneas, $nightly.
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
  "$aeneas" -backend lean -dest "$out/lean" "$out/crate"/*.llbc || { echo "aeneas failed on $name"; return 1; }
  echo "--- generated Lean for $name"
  find "$out/lean" -name '*.lean' | sort
  return 0
}

stage_aeneas() {
  [ -d "$work/charon/.git" ] || die "charon is not fetched; run the charon stage first"
  [ -x "$work/aeneas/charon/bin/charon" ] || die "charon is not built; run the charon stage first"
  check_pin aeneas "$work/aeneas" "$AENEAS_REV" "$AENEAS_TREE_SHA256"
  local charon_rev
  charon_rev="$(charon_pin_rev)"
  check_pin charon "$work/charon" "$charon_rev" "$CHARON_TREE_SHA256"

  use_switch
  # opam reads the dependencies from the opam file in src/. It needs the path
  # (./src), not the bare name, which it takes for a package called src.
  if ! ls "$work"/aeneas/src/*.opam >/dev/null 2>&1; then
    echo "no opam file in aeneas/src; its dune-project says:" >&2
    head -n 60 "$work/aeneas/src/dune-project" >&2 || true
    die "cannot read the OCaml dependencies of Aeneas"
  fi
  (cd "$work/aeneas" && opam install ./src --deps-only --with-test --yes)
  (cd "$work/aeneas" && make)
  echo "== aeneas built"
  "$work/aeneas/bin/aeneas" -version || true

  charon="$work/aeneas/charon/bin/charon"
  aeneas="$work/aeneas/bin/aeneas"
  nightly="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$work/charon/rust-toolchain")"
  [ -n "$nightly" ] || die "no channel in charon/rust-toolchain"
  echo "== charon's toolchain: $nightly"
  rustup toolchain install "$nightly" --profile minimal -c rustc-dev -c llvm-tools -c rust-src

  # The loop crate must translate; its failure ends the stage.
  translate sum_u32 porting/aeneas/spike || die "the loop crate did not translate"

  # The f32 crate is reported, not required: its outcome is the f32 verdict.
  if translate dot_f32 porting/aeneas/spike-f32; then
    echo translated > "$work/f32.translate"
  else
    echo "not translated" > "$work/f32.translate"
    echo "f32 (dot_f32) did not translate; see the log above"
  fi

  # A probe of f32 arithmetic with no float literal, since dot_f32 failed at its
  # literal. Its outcome is the second part of the f32 verdict; it is reported,
  # not required to pass.
  if translate dot_f32_from porting/aeneas/spike-f32-probe; then
    echo translated > "$work/f32probe.translate"
    echo "f32 probe (dot_f32_from): translated"
  else
    echo "not translated" > "$work/f32probe.translate"
    echo "f32 probe (dot_f32_from) did not translate; see the log above"
  fi

  # The TopKFilter attempt (D-331): the heap loops on an integer weight, the
  # shape the P3-2 keystone would need extracted. Reported, not required: the
  # outcome, and the stage that failed if it did, goes into D-331.
  if translate topk_i64 porting/aeneas/spike-topk; then
    echo translated > "$work/topk.translate"
    echo "topk (topk_i64): translated"
  else
    echo "not translated" > "$work/topk.translate"
    echo "topk (topk_i64) did not translate; see the log above"
  fi
}

# Module names of the generated Lean files under a directory, as a lakefile list.
modules_of() { # dir
  (cd "$1" && find . -name '*.lean' | sed 's#^\./##; s#\.lean$##; s#/#.#g' | sort | sed 's/.*/"&"/' | paste -sd, -)
}

stage_lean() {
  [ -d "$work/aeneas/backends/lean" ] || die "the Aeneas Lean library is not fetched; run the charon stage first"
  [ -d "$work/out-sum_u32/lean" ] || die "the loop crate is not translated; run the aeneas stage first"
  command -v lake >/dev/null || die "lake is not on the PATH; the elan step did not run"

  # Both crates go into one Lake project, so Mathlib is fetched and built once.
  # Each crate is its own library, so the f32 build cannot fail the loop build.
  local proj="$work/lean-proj" loop_mods f32_mods="" f32_block="" f32_built="no"
  local topk_mods="" topk_block="" topk_built="no"
  rm -rf "$proj"
  mkdir -p "$proj"
  cp "$work/aeneas/backends/lean/lean-toolchain" "$proj/lean-toolchain"
  cp -r "$work/out-sum_u32/lean"/. "$proj/"
  loop_mods="$(modules_of "$work/out-sum_u32/lean")"
  if [ "$(cat "$work/f32.translate" 2>/dev/null)" = "translated" ]; then
    cp -r "$work/out-dot_f32/lean"/. "$proj/"
    f32_mods="$(modules_of "$work/out-dot_f32/lean")"
    f32_block=$(printf '\n[[lean_lib]]\nname = "SpikeF32"\nsrcDir = "."\nroots = [%s]\n' "$f32_mods")
  fi
  if [ "$(cat "$work/topk.translate" 2>/dev/null)" = "translated" ]; then
    cp -r "$work/out-topk_i64/lean"/. "$proj/"
    topk_mods="$(modules_of "$work/out-topk_i64/lean")"
    topk_block=$(printf '\n[[lean_lib]]\nname = "SpikeTopK"\nsrcDir = "."\nroots = [%s]\n' "$topk_mods")
  fi

  cat > "$proj/lakefile.toml" <<EOF
name = "spike"

[[require]]
name = "aeneas"
path = "$work/aeneas/backends/lean"

[[lean_lib]]
name = "SpikeLoop"
srcDir = "."
roots = [$loop_mods]
$f32_block
$topk_block
EOF

  # Mathlib is a dependency of the Aeneas Lean library. Its prebuilt files come
  # from the Mathlib cache; building Mathlib here would take hours.
  echo "== lean cache: fetching the Mathlib build"
  (cd "$proj" && lake exe cache get) || die "lake exe cache get failed; Mathlib is not fetched"

  echo "== lake build SpikeLoop (modules: $loop_mods)"
  echo "disk before lake build: $(df -h / | tail -n 1)"
  (cd "$proj" && lake build SpikeLoop) || die "the loop crate did not build"
  echo "loop (sum_u32): translated and built"

  if [ -n "$f32_mods" ]; then
    echo "== lake build SpikeF32 (modules: $f32_mods)"
    if (cd "$proj" && lake build SpikeF32); then
      f32_built="yes"
    fi
  fi
  if [ "$f32_built" = "yes" ]; then
    echo "f32 (dot_f32): translated and built"
  else
    echo "f32 (dot_f32): not built; see the log above"
  fi

  # The TopKFilter attempt (D-331). Reported, not required: a failure here is
  # recorded in D-331 with the failing stage and its error, and does not fail CI.
  if [ -n "$topk_mods" ]; then
    echo "== lake build SpikeTopK (modules: $topk_mods)"
    if (cd "$proj" && lake build SpikeTopK); then
      topk_built="yes"
    fi
  fi
  if [ "$topk_built" = "yes" ]; then
    echo "topk (topk_i64): extracted to Lean and built"
  else
    echo "topk (topk_i64): not built; see the log above"
  fi
}

case "$mode" in
  charon)
    stage_charon
    ;;
  aeneas)
    stage_aeneas
    ;;
  lean)
    stage_lean
    ;;
  spike)
    stage_charon
    stage_aeneas
    stage_lean
    ;;
esac
