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

## Head mode (D-325)

`PARITY_TREE=head scripts/parity.sh run metrics:except=pinterest_20_bpr` (and `metrics:only=pinterest_20_bpr`) runs the metrics suites against this checkout's Rust crates. The Go oracle is still built from `2e9c969`.

- The Rust side is an overlay at `target/parity/head`. It is the pinned tree, which keeps the Go module, `conformance/` and `contracts/`, with this checkout's `crates/`, `third_party/`, `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml` in its place. The overlay's `Cargo.toml` adds `conformance` to the workspace members. Nothing is written into this tree.
- Head mode takes only the metrics suites. The other suites stay on the pinned snapshot, and `ctr_replay` is refused, because its golden depends on the CPU path (D-319).
- CI runs head mode on release and on dispatch only (`metrics-head` and `metrics-head-pinterest` in `.github/workflows/parity.yml`), so it does not run per PR.
- The frozen metrics suite is not edited to fit the head. Where it does not compile, or a result differs, the result is recorded in `PORTING.md`.
