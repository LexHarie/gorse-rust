#!/usr/bin/env bash
# G4, the statement freeze of the verification plan, for the Lean project in proofs/.
#
# proofs/statements.txt lists every theorem and lemma under proofs/Gorse, with a
# sha256 of its statement: the declaration's text from its keyword up to the ":="
# that opens the proof, with whitespace collapsed. Each entry also names the
# decision-log ID that accepted the statement, and that ID must appear in PORTING.md.
#
# A changed statement, a new theorem or a removed one fails the check until the
# manifest is rewritten with --record and a decision-log ID. The hash covers the
# text, so a change of layout inside a statement counts as a change too.
#
# Usage:
#   scripts/check-statements.sh                  check proofs/ against the manifest
#   scripts/check-statements.sh --record D-NNN   rewrite the manifest; statements that
#                                                changed or are new get D-NNN
#   scripts/check-statements.sh --self-test      check that a changed statement, a new
#                                                theorem, a removed one and an unknown
#                                                decision ID each fail
#
# Exits non-zero when a check fails.
set -euo pipefail

cd "$(dirname "$0")/.."

ROOT=proofs
MANIFEST=proofs/statements.txt

# Prints "hash rel name" for every theorem or lemma under $1/Gorse, sorted by file
# and name. rel is the path relative to $1.
current_statements() {
  local root="$1" file rel
  local -a files
  mapfile -t files < <(find "$root/Gorse" -name '*.lean' | LC_ALL=C sort)
  for file in "${files[@]}"; do
    rel="${file#"$root"/}"
    awk -v rel="$rel" '
      function emit(text,    s) {
        s = text
        gsub(/[[:space:]]+/, " ", s)
        sub(/^ /, "", s)
        sub(/ $/, "", s)
        print rel "\t" name "\t" s
      }
      /^(theorem|lemma)[[:space:]]/ && !open { name = $2; buf = ""; open = 1 }
      open {
        idx = index($0, ":=")
        if (idx > 0) {
          buf = buf " " substr($0, 1, idx - 1)
          emit(buf)
          open = 0
        } else {
          buf = buf " " $0
        }
      }
      END {
        if (open) { print "G4: no := ends the statement of " name " in " rel > "/dev/stderr"; exit 1 }
      }
    ' "$file"
  done | while IFS=$'\t' read -r rel name text; do
    printf '%s %s %s\n' "$(printf '%s' "$text" | sha256sum | cut -d' ' -f1)" "$rel" "$name"
  done | LC_ALL=C sort -k2,2 -k3,3
}

# Compares the statements under root $1 with manifest $2. Prints each mismatch and
# returns 1 if there is any.
check_manifest() {
  local root="$1" manifest="$2" current hash rel name decision key status=0
  local -A want_hash=() want_decision=() have=()
  while read -r hash rel name decision; do
    [ -z "${hash:-}" ] && continue
    case "$hash" in \#*) continue ;; esac
    key="$rel $name"
    want_hash["$key"]="$hash"
    want_decision["$key"]="$decision"
  done < "$manifest"

  current="$(current_statements "$root")"
  while read -r hash rel name; do
    [ -z "${hash:-}" ] && continue
    key="$rel $name"
    have["$key"]=1
    if [ -z "${want_hash[$key]:-}" ]; then
      echo "G4: $key is not in the manifest; freeze it with --record D-NNN" >&2
      status=1
    elif [ "${want_hash[$key]}" != "$hash" ]; then
      echo "G4: the statement of $key changed; give its entry a new decision-log ID" >&2
      status=1
    fi
  done <<< "$current"

  for key in "${!want_hash[@]}"; do
    if [ -z "${have[$key]:-}" ]; then
      echo "G4: $key is in the manifest but no longer in the sources" >&2
      status=1
    fi
  done

  for key in "${!want_decision[@]}"; do
    decision="${want_decision[$key]}"
    if ! grep -q -E -- "(^|[^0-9])${decision}([^0-9]|\$)" PORTING.md; then
      echo "G4: decision $decision (for $key) is not in PORTING.md" >&2
      status=1
    fi
  done
  return "$status"
}

# Rewrites the manifest for the current statements. A statement whose hash is
# unchanged keeps its decision ID; a changed or new one gets $1.
record() {
  local decision="$1" current hash rel name d key
  local -A old_hash=() old_decision=()
  [[ "$decision" =~ ^D-[0-9]+$ ]] || { echo "G4: $decision is not a decision-log ID (D-NNN)" >&2; exit 2; }
  grep -q -E -- "(^|[^0-9])${decision}([^0-9]|\$)" PORTING.md \
    || { echo "G4: $decision is not in PORTING.md; record the decision first" >&2; exit 1; }

  if [ -f "$MANIFEST" ]; then
    while read -r hash rel name d; do
      [ -z "${hash:-}" ] && continue
      case "$hash" in \#*) continue ;; esac
      key="$rel $name"
      old_hash["$key"]="$hash"
      old_decision["$key"]="$d"
    done < "$MANIFEST"
  fi

  current="$(current_statements "$ROOT")"
  {
    echo "# G4 statement manifest, checked by scripts/check-statements.sh."
    echo "# One line per theorem: sha256 of its statement, file, name, and the"
    echo "# decision-log ID that accepted the statement."
    while read -r hash rel name; do
      [ -z "${hash:-}" ] && continue
      key="$rel $name"
      if [ "${old_hash[$key]:-}" = "$hash" ]; then
        d="${old_decision[$key]}"
      else
        d="$decision"
      fi
      echo "$hash $rel $name $d"
    done <<< "$current"
  } > "$MANIFEST.tmp"
  mv "$MANIFEST.tmp" "$MANIFEST"
  echo "G4: wrote $MANIFEST"
}

# Each mutation of a copy must fail the check, and the untouched copy must pass.
self_test() {
  local work
  work="$(mktemp -d)"
  # Expanded now, because the trap runs after this function's locals are gone.
  trap "rm -rf '$work'" EXIT

  fresh() {
    rm -rf "$work/proofs"
    mkdir -p "$work/proofs"
    cp -R "$ROOT/." "$work/proofs/"
  }
  expect_fail() {
    local what="$1"
    shift
    if "$@" >/dev/null 2>&1; then
      echo "self-test: $what passed the check; the check is wrong" >&2
      exit 1
    fi
    echo "self-test: $what fails, as required"
  }
  mutated() {
    # A mutation that changes nothing would make its test vacuous.
    if cmp -s "$ROOT/$1" "$work/proofs/$1"; then
      echo "self-test: the edit to $1 changed nothing; the test is wrong" >&2
      exit 1
    fi
  }

  fresh
  if ! check_manifest "$work/proofs" "$work/proofs/statements.txt" >/dev/null 2>&1; then
    echo "self-test: the clean copy failed the check; the check is wrong" >&2
    exit 1
  fi
  echo "self-test: the clean copy passes"

  fresh
  sed -i 's/^theorem scaffold_true : True := trivial$/theorem scaffold_true : True ∧ True := trivial/' \
    "$work/proofs/Gorse/Scaffold.lean"
  mutated Gorse/Scaffold.lean
  expect_fail "a changed statement" check_manifest "$work/proofs" "$work/proofs/statements.txt"

  fresh
  printf '\ntheorem planted : True := trivial\n' >> "$work/proofs/Gorse/Scaffold.lean"
  mutated Gorse/Scaffold.lean
  expect_fail "a new theorem" check_manifest "$work/proofs" "$work/proofs/statements.txt"

  fresh
  sed -i '/^theorem scaffold_or_comm /d' "$work/proofs/Gorse/Scaffold.lean"
  mutated Gorse/Scaffold.lean
  expect_fail "a removed theorem" check_manifest "$work/proofs" "$work/proofs/statements.txt"

  fresh
  sed -i 's/ D-316$/ D-999/' "$work/proofs/statements.txt"
  mutated statements.txt
  expect_fail "an unknown decision ID" check_manifest "$work/proofs" "$work/proofs/statements.txt"
}

case "${1:-}" in
  "")
    check_manifest "$ROOT" "$MANIFEST"
    echo "G4: every statement under $ROOT/Gorse matches its manifest entry"
    ;;
  --record)
    [ -n "${2:-}" ] || { echo "usage: $0 --record D-NNN" >&2; exit 2; }
    record "$2"
    ;;
  --self-test)
    self_test
    ;;
  *)
    echo "usage: $0 [--record D-NNN | --self-test]" >&2
    exit 2
    ;;
esac
