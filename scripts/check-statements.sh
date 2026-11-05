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
# With --base REV the check also reads the manifest at REV, the base of a change.
# A statement that is new or changed against REV must then carry a decision-log ID
# that REV's manifest does not cite, so an edit cannot keep the ID that accepted the
# old statement. Without --base only the rules above apply.
#
# Usage:
#   scripts/check-statements.sh                  check proofs/ against the manifest
#   scripts/check-statements.sh --base REV       the same, and a new or changed statement
#                                                needs an ID that REV's manifest does not cite
#   scripts/check-statements.sh --record D-NNN   rewrite the manifest; statements that
#                                                changed or are new get D-NNN
#   scripts/check-statements.sh --self-test      check that a changed statement, a new
#                                                theorem, a removed one, an unknown decision
#                                                ID and a reused decision ID each fail, and
#                                                that a new decision ID passes
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

# Reads the entries of manifest $1 into the associative arrays named $2 (the hash) and
# $3 (the decision-log ID), both keyed by "file name".
read_entries() {
  local manifest="$1" hash rel name decision key
  local -n hashes="$2" decisions="$3"
  while read -r hash rel name decision; do
    [ -z "${hash:-}" ] && continue
    case "$hash" in \#*) continue ;; esac
    key="$rel $name"
    hashes["$key"]="$hash"
    decisions["$key"]="$decision"
  done < "$manifest"
}

# Compares the statements under root $1 with manifest $2. With a third argument, a
# manifest file for the base, each statement that is new or changed against the base
# must carry a decision-log ID the base does not cite. Prints each mismatch and
# returns 1 if there is any.
check_manifest() {
  local root="$1" manifest="$2" base="${3:-}" current hash rel name decision key status=0
  local -A want_hash=() want_decision=() have=() base_hash=() base_decision=() base_cited=()
  read_entries "$manifest" want_hash want_decision

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

  if [ -n "$base" ]; then
    read_entries "$base" base_hash base_decision
    for key in "${!base_decision[@]}"; do
      base_cited["${base_decision[$key]}"]=1
    done
    for key in "${!want_decision[@]}"; do
      decision="${want_decision[$key]}"
      if [ "${base_hash[$key]:-}" != "${want_hash[$key]}" ] && [ -n "${base_cited[$decision]:-}" ]; then
        echo "G4: the statement of $key is new or changed against the base, but its decision ID $decision is already cited there; give it an ID the base does not cite" >&2
        status=1
      fi
    done
  fi
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
  # The committed manifest stands in for the base of a change.
  cp "$MANIFEST" "$work/base.txt"

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
  expect_pass() {
    local what="$1"
    shift
    if ! "$@" >/dev/null 2>&1; then
      echo "self-test: $what failed the check; the check is wrong" >&2
      exit 1
    fi
    echo "self-test: $what passes, as required"
  }
  mutated() {
    # A mutation that changes nothing would make its test vacuous.
    if cmp -s "$ROOT/$1" "$work/proofs/$1"; then
      echo "self-test: the edit to $1 changed nothing; the test is wrong" >&2
      exit 1
    fi
  }
  # Sets the manifest's hash for the theorem named $1 to the hash of its current
  # statement, and keeps the decision ID. This is what --record does, without the ID.
  rehash() {
    local new
    new="$(current_statements "$work/proofs" | awk -v n="$1" '$3 == n { print $1 }')"
    sed -i "s/^[0-9a-f]* \(Gorse\/Scaffold.lean $1 \)/$new \1/" "$work/proofs/statements.txt"
    mutated statements.txt
  }

  fresh
  if ! check_manifest "$work/proofs" "$work/proofs/statements.txt" >/dev/null 2>&1; then
    echo "self-test: the clean copy failed the check; the check is wrong" >&2
    exit 1
  fi
  echo "self-test: the clean copy passes"
  expect_pass "the clean copy against the base" \
    check_manifest "$work/proofs" "$work/proofs/statements.txt" "$work/base.txt"

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

  # The edit keeps the ID that accepted the old statement, and the manifest is
  # updated to match. Only the comparison with the base can catch this.
  fresh
  sed -i 's/^theorem scaffold_true : True := trivial$/theorem scaffold_true : True ∧ True := trivial/' \
    "$work/proofs/Gorse/Scaffold.lean"
  mutated Gorse/Scaffold.lean
  rehash scaffold_true
  expect_fail "a changed statement under the ID the base cites" \
    check_manifest "$work/proofs" "$work/proofs/statements.txt" "$work/base.txt"

  # The same edit under an ID that PORTING.md records and the base does not cite.
  sed -i 's/ scaffold_true D-316$/ scaffold_true D-315/' "$work/proofs/statements.txt"
  expect_pass "a changed statement under a new decision ID" \
    check_manifest "$work/proofs" "$work/proofs/statements.txt" "$work/base.txt"

  # A new theorem added under an ID the base already cites.
  fresh
  printf '\ntheorem planted : True := trivial\n' >> "$work/proofs/Gorse/Scaffold.lean"
  mutated Gorse/Scaffold.lean
  current_statements "$work/proofs" | awk '$3 == "planted" { print $1, $2, $3, "D-316" }' >> "$work/proofs/statements.txt"
  expect_fail "a new theorem under a base-cited ID" \
    check_manifest "$work/proofs" "$work/proofs/statements.txt" "$work/base.txt"
}

case "${1:-}" in
  "")
    check_manifest "$ROOT" "$MANIFEST"
    echo "G4: every statement under $ROOT/Gorse matches its manifest entry"
    ;;
  --base)
    [ -n "${2:-}" ] || { echo "usage: $0 --base REV" >&2; exit 2; }
    git rev-parse --verify -q "$2^{commit}" >/dev/null \
      || { echo "G4: $2 is not a commit in this clone; fetch the base first" >&2; exit 2; }
    base="$(mktemp)"
    trap "rm -f '$base'" EXIT
    # A base with no manifest yet has no decision IDs, so every statement is new.
    if git cat-file -e "$2:$MANIFEST" 2>/dev/null; then
      git show "$2:$MANIFEST" > "$base"
    else
      : > "$base"
    fi
    check_manifest "$ROOT" "$MANIFEST" "$base"
    echo "G4: every statement under $ROOT/Gorse matches its manifest entry, and each new or changed one has a decision ID that $2 does not cite"
    ;;
  --record)
    [ -n "${2:-}" ] || { echo "usage: $0 --record D-NNN" >&2; exit 2; }
    record "$2"
    ;;
  --self-test)
    self_test
    ;;
  *)
    echo "usage: $0 [--base REV | --record D-NNN | --self-test]" >&2
    exit 2
    ;;
esac
