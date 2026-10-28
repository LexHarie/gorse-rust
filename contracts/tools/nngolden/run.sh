#!/bin/sh
# Writes crates/gorse-common/tests/data/nn_golden.json. Run from the
# repository root on linux/amd64 with a CPU that has AVX-512:
#
#	contracts/tools/nngolden/run.sh
#
# The tool needs a few unexported functions of common/nn, which export.go.txt
# exports. `go run -overlay` adds that file to the package for this build only.
set -eu
root=$(pwd)
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
cat >"$dir/overlay.json" <<EOF
{"Replace": {"$root/common/nn/zz_nngolden_export.go": "$root/contracts/tools/nngolden/export.go.txt"}}
EOF
go build -tags nngolden -overlay "$dir/overlay.json" -o "$dir/nngolden" ./contracts/tools/nngolden
"$dir/nngolden" -out crates/gorse-common/tests/data/nn_golden.json
