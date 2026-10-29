#!/usr/bin/env bash
# Vendors the pinned dashboard bundle into crates/gorse-master/dashboard.
# Run from anywhere; it works from the repository root. See main.go.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"
go run ./contracts/tools/dashboard -out crates/gorse-master/dashboard
