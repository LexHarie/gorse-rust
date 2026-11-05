# Parity tier (G11)

The frozen differential suites of the Go oracle live in the pinned tree, not
here. They are `conformance/tests` at commit `2e9c969` (branch
`go-oracle-final`). Their Go inputs resolve from their own tree
(`CARGO_MANIFEST_DIR/..`), so a copy here would not run. The suites therefore
run from an extracted copy of that commit, and this tree gains no test:

- `scripts/parity.sh build` extracts `git archive 2e9c969`, builds the Go nodes
  with Go 1.27.0 (the version `go.mod` names), and records their SHA-256
  digests in `target/parity/bin/SHA256SUMS`.
- `scripts/parity.sh dry-run` compiles the suites and lists them; it starts no
  backend.
- `scripts/parity.sh run [NAME...]` runs the suites with `--ignored`. It needs
  the backends of `storage/docker-compose.yml`, so it runs in CI
  (`.github/workflows/parity.yml`), per release and on demand.

Environment limits are recorded in `PORTING.md`.
