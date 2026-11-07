#!/usr/bin/env bash
# The disassembly check of the P4 dot kernels (D-344, D-345, D-346, D-347).
#
# The three dot models depend on which operations the compiler fuses into an FMA. This
# script compiles the kernels with clang and the flags crates/gorse-kernels/build.rs uses,
# and checks the fusion each model assumes:
#
#   _mm256_dot (floats_avx.c)     no vfmadd or vfmsub: the AVX file has no FMA instruction set,
#                                 so every product and sum rounds separately (D-345)
#   _mm512_dot (floats_avx512.c)  the vector epochs use vfmadd*ps, and the scalar tail uses
#                                 vfmadd*ss, one fused step per element (D-344)
#   vdot (floats_neon.c, aarch64) the epochs use fmla, and the scalar tail uses fmadd: each
#                                 is one fused step (D-346)
#
# Usage: scripts/check-disasm.sh [work-directory]
#
# DISASM_EXTRA_FLAGS adds flags to every compile. A negative test sets -ffp-contract=off,
# and the check must then fail.
set -euo pipefail

cd "$(dirname "$0")/.."

work="${1:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"

CC="${CC:-clang}"
extra="${DISASM_EXTRA_FLAGS:-}"
echo "clang: $("$CC" --version | head -1)"

# The common flags of build.rs (lines 60-75), then the goat flags of the target.
common=(-fno-jump-tables -mllvm -inline-threshold=1000 -fno-asynchronous-unwind-tables
  -fno-exceptions -fno-builtin -ffreestanding)

# compile SOURCE OUTPUT FLAGS...: writes the assembly of SOURCE.
compile() {
  local src="$1" out="$2"
  shift 2
  # shellcheck disable=SC2086
  "$CC" "$@" "${common[@]}" $extra -S -o "$out" "$src"
}

# body FILE SYMBOL: prints the body of SYMBOL, from its label to the next function label.
body() {
  awk -v s="$2" '
    $0 ~ "^" s ":" { on = 1; print; next }
    on && /^[A-Za-z_][A-Za-z0-9_]*:/ { exit }
    on && /^\.Lfunc_end/ { exit }
    on { print }
  ' "$1"
}

# count PATTERN: the number of lines of standard input that match the extended regex.
count() {
  grep -c -E "$1" || true
}

compile common/floats/src/floats_avx.c "$work/avx.s" \
  -O3 -mavx -mf16c -mno-red-zone -mstackrealign --target=x86_64-unknown-linux-gnu
compile common/floats/src/floats_avx512.c "$work/avx512.s" \
  -O3 -mavx -mfma -mavx512f -mno-red-zone -mstackrealign --target=x86_64-unknown-linux-gnu
compile common/floats/src/floats_neon.c "$work/neon.s" \
  -O3 -ffixed-x18 --target=aarch64-unknown-linux-gnu

fail=0

# expect_at_least LABEL COUNT MIN: the check passes when COUNT is at least MIN.
expect_at_least() {
  if [ "$2" -ge "$3" ]; then
    echo "ok    $1: $2 (at least $3)"
  else
    echo "FAIL  $1: $2, expected at least $3"
    fail=1
  fi
}

# expect_none LABEL COUNT: the check passes when COUNT is zero.
expect_none() {
  if [ "$2" -eq 0 ]; then
    echo "ok    $1: 0"
  else
    echo "FAIL  $1: $2, expected none"
    fail=1
  fi
}

fma_x86='vfn?m(add|sub)[0-9]*(ps|pd|ss|sd)'

# D-345: the AVX dot has no fused operation.
avx_fma=$(body "$work/avx.s" _mm256_dot | count "$fma_x86")
expect_none "_mm256_dot fused instructions (D-345)" "$avx_fma"

# D-344: the AVX-512 dot has fused vector epochs and a fused scalar tail.
avx512_ps=$(body "$work/avx512.s" _mm512_dot | count 'vfn?madd[0-9]*ps')
avx512_ss=$(body "$work/avx512.s" _mm512_dot | count 'vfn?madd[0-9]*ss')
expect_at_least "_mm512_dot vfmadd*ps epochs (D-344)" "$avx512_ps" 1
expect_at_least "_mm512_dot vfmadd*ss tail (D-344)" "$avx512_ss" 1

# D-346: the NEON dot has fused epochs (fmla) and a fused scalar tail (fmadd).
neon_fmla=$(body "$work/neon.s" vdot | count '\bfmla\b')
neon_fmadd=$(body "$work/neon.s" vdot | count '\bfmadd\b')
expect_at_least "vdot fmla epochs (D-346)" "$neon_fmla" 1
expect_at_least "vdot fmadd tail (D-346)" "$neon_fmadd" 1

if [ "$fail" -ne 0 ]; then
  echo "disassembly check failed"
  exit 1
fi
echo "disassembly check passed"
