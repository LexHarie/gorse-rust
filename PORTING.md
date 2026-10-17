# Porting Gorse to Rust

## Upstream pin

| | |
|---|---|
| Upstream | https://github.com/gorse-io/gorse |
| Commit | `add21b53123508e00bdc02bd92290b08603b0236` (`ci: add GitHub Actions lockfile (#1403)`, 2026-10-16T00:13:05+08:00) |
| Go toolchain used as the oracle | go1.27.0 linux/amd64 (`go.mod` says `go 1.27.0`) |
| Rust toolchain | 1.99.1 stable (`rustc 1.99.1 (58cf8f9c3 2026-10-13)`), edition 2024, pinned in `rust-toolchain.toml` |

The Go tree at that commit stays in this repository, untouched, next to the Rust
workspace. It is the oracle: conformance tests build the Go binaries from it.
Every Rust module names the Go files it ports with a
`// Ports: <go file> @ add21b5` header.

## Layout

| Path | Content |
|---|---|
| `contracts/` | Frozen contracts extracted in Phase 0 (`contracts/README.md`) |
| `porting/` | Dependency graph, crate selection, work packages |
| `crates/` | The Rust workspace, one crate per Go package group |
| `conformance/` | Differential tests against the Go build |
| `PORTING_NOTES.md` | Go quirks reproduced, deviations, open questions |

## Status board

| Phase | State | Notes |
|---|---|---|
| 0. Contracts | done | Exit gate decided under D-1; see "Phase 0 evidence" |
| 1. Foundation | not started | WP-1-1 to WP-1-18 in `porting/work-packages.md` |
| 2. Storage | not started | WP-2-1 to WP-2-13 |
| 3. Models and logics | not started | WP-3-1 to WP-3-8; metric parity blocked on datasets (O-1) |
| 4. Nodes | not started | WP-4-1 to WP-4-7 |
| 5. Conformance | not started | WP-5-1 to WP-5-3 |

Each work package's section 8 report goes in the message of the commit that
completes it, and its line here moves to done.

### Phase 0 evidence

- `contracts/rest/*.openapi.json` equal the live `/apidocs.json` of a Go
  `gorse-server` and `gorse-master` built from the pinned commit, except the order
  of `required` arrays (`PORTING_NOTES.md` Q-8).
- `contracts/schema/*` are dumps of databases initialised by the Go stores against
  MySQL 8.0.46, PostgreSQL 10.0, MongoDB 6.0.28, Redis 8.10.2 and SQLite.
- `contracts/proto/SHA256SUMS` matches `protocol/*.proto`.
- `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`,
  `cargo fmt --check` and `cargo test --workspace` pass on the skeleton.

## Plan corrections

Ritchie asked for the plan to be corrected where evidence shows it is wrong
(2026-10-17, "I may have made mistakes regarding the project plan, feel free to
autonomously update your worldview ... but cheating is not allowed"). Each
correction below states the evidence.

- **C-1 Codebase size.** The plan says ~27,500 hand-written Go lines. Counted the
  plan's way (non-test `.go`, `*.pb.go` excluded) the pinned commit has 37,678
  lines, 31,488 without blank and comment-only lines, and the plan's own
  per-package table sums to about 37,600. The other rows check out: tests 20,832,
  generated protobuf 10,130, SIMD assembly 10,864 (`find ... | xargs wc -l`).
- **C-2 A one-to-one crate mapping is cyclic.** `config` imports the `storage`
  root package and `storage/data` imports `config`; `storage/meta` imports `model`
  and `model/cf` imports `storage/meta` (`porting/dependency-graph.md`). Two small
  crates break the cycles: `gorse-storage-core` (`storage/*.go`) and
  `gorse-model-core` (`model/*.go`). `cargo build` confirms the crate graph is
  acyclic.
- **C-3 The default vector store is missing from the plan.** `xvec://` (the
  embedded `github.com/gorse-io/xvec`, 102,923 Go lines) is the default
  `database.vector_store`. See D-6.
- **C-4 The default FM model is not XLA.** `model/ctr/fm.go` (`//go:build !cgo ||
  !xla`) on the hand-written `common/nn` is what every default build runs; XLA
  (`fm_xla.go`) is only compiled with `-tags xla`, used by the `Dockerfile.cuda`
  images. See D-8.
- **C-5 The REST contract is OpenAPI 2.0.** go-restful-openapi serves Swagger 2.0;
  the proposed `utoipa` only emits OpenAPI 3.x, so it cannot reproduce
  `/apidocs.json`. See D-15.
- **C-6 Model blobs use `encoding/gob`**, which the crate table does not cover. See
  D-7 and `contracts/model-blobs.md`.
- **C-7 `dataset` needs `storage/data` record types**, so Phase 1 cannot finish
  `dataset` without part of `storage/data`. WP-1-17 moves the record types into
  Phase 1.
- **C-8 Dependencies the crate table misses**: `araddon/dateparse` (cache times),
  `lafikl/consistent` (worker partitioning, a cluster contract), `juju/ratelimit`,
  `gorilla/securecookie`, `jellydator/ttlcache`, the dashboard bundle
  (`github.com/gorse-io/dashboard`, a zip inside a `statik` file), and the system
  `protoc` the plan's codegen assumes (not installed; `protox` used instead).
  All are in `porting/crate-selection.md`.
- **C-9 Repository host.** The plan says GitLab with one merge request per work
  package; the configured repository is GitHub, and Ritchie said not to open PRs.
  See D-2.
- **C-10 Where the SDK suites live.** The Go SDK suite is `client/client_test.go`
  in this tree, which drives `github.com/gorse-io/gorse-go` against a running
  cluster (the Go CI's integrate job, matrix `mysql+qdrant`, `postgres+weaviate`,
  `mongo+milvus`, `sqlite+xvec`). The Rust SDK is the `gorse_rs` crate (0.5.0 on
  crates.io).

## Decision log

Ritchie's instruction of 2026-10-17 02:58Z, "btw don't do PRs. Just
autonomously build it until finish", removes the human approval gates. The
decisions the plan reserves for the human (section 9) are therefore taken here,
each with its reasoning, so they can be audited. None of them makes a mixed
Go/Rust cluster unsafe, drops a storage backend, adds a non-permissive licence,
or accepts a metric gap outside the Go tests' own tolerances.

- **D-1 Gates.** Phase exit gates that need human approval are passed by this log
  instead. Evidence requirements are unchanged.
- **D-2 Repository and workflow.** Work goes to the configured GitHub repository
  `LexHarie/gorse-rust` on branch `claude/phase-0-contracts-lqc6fr`, the branch
  this session is allowed to push. No pull requests. `master` is left for Ritchie
  to merge into, because pushing it needs his explicit say-so. CI will be GitHub
  Actions with the jobs the plan lists for GitLab CI (fmt, clippy, unit tests,
  storage tests against `docker-compose`).
- **D-3 Crates.** As in `porting/crate-selection.md`, latest stable versions, all
  permissive licences.
- **D-4 Toolchain.** Rust 1.99.1, edition 2024, resolver 3. It is the latest
  stable release: `https://static.rust-lang.org/dist/channel-rust-stable.toml`
  (dated 2026-10-15) lists `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
- **D-5 Workspace.** The plan's crates plus `gorse-storage-core` and
  `gorse-model-core` (C-2), and `gorse-client` for the Go `client` package when
  WP-4-6 needs it.
- **D-6 xvec.** The Rust master implements `xvec://` natively with its own file
  format instead of porting `gorse-io/xvec`. Reasons: xvec is 102,923 lines, about
  2.7 times Gorse itself, with HNSW, IVF, DiskANN, full-text search and a SQL
  engine Gorse never uses; its contents are derived data the master rebuilds on
  its first training round; and workers and servers only reach it through the
  master's `VectorStore` gRPC service, which is reproduced exactly. Logged as
  deviation X-1.
- **D-7 gob.** A hand-written codec for the gob subset in
  `contracts/model-blobs.md`, tested against Go-produced bytes. The `gob` crate is
  an unmaintained 2018 prototype.
- **D-8 FM model.** Port `fm.go` and `common/nn` by hand. The XLA variant is not
  ported: its blobs are not readable by default Go builds anyway (Q-18), and
  `candle`/`burn` would change numerics and the blob layout. CUDA acceleration is
  out of scope for wire compatibility.
- **D-9 Random numbers.** Port Go's `math/rand` generator so seeded tests produce
  the Go sequences (risk 2).
- **D-10 Expressions.** WP-3-6 first builds the corpus of expressions from the Go
  tests and docs. The `expr-lang` crate is kept only if it evaluates every corpus
  entry like Go; otherwise a hand-written evaluator for the accepted subset is
  used.
- **D-11 Dates.** Hand port of `dateparse.ParseAny` (risk 7).
- **D-12 Blob names.** Reject unsafe blob names instead of reproducing the Go path
  traversal (X-2). Reproducing it would make a mixed cluster less safe to run.
- **D-13 Nil `scan_options`.** Answer with an error instead of crashing the
  master (X-3), for the same reason.
- **D-14 Go runtime endpoints.** `/debug/pprof/*` answer 404 and no `go_*` metrics
  are exported (X-4, X-5); nothing in a Rust process can produce them.
- **D-15 OpenAPI.** Serve the frozen `contracts/rest/*.openapi.json` at
  `/apidocs.json`; serve Swagger UI from the assets of the pinned
  `swaggest/swgui` module.
- **D-16 Dashboard.** Embed the exact zip from the pinned
  `github.com/gorse-io/dashboard@v0.5.20260714` module, fetched through the Go
  module proxy and checked against `go.sum`; it is extracted at build time and not
  committed.
- **D-17 Milvus and Weaviate.** Hand-written adapters over their REST APIs
  (`milvus-sdk-rust` has a non-standard licence; `weaviate-community` is
  unmaintained).
- **D-18 SQLite.** `rusqlite` for the SQLite data, cache and meta stores, `sqlx`
  for MySQL and Postgres (custom `json_contains` function, Q-25).
- **D-19 Backends.** Every backend stays: MySQL (and MariaDB), Postgres, SQLite,
  MongoDB, Redis (standalone, cluster, Kvrocks), Qdrant, Milvus, Weaviate, xvec,
  S3, GCS, Azure, local directory and master-served blobs.
- **D-20 Metric tolerance.** The tolerance is what each Go model test asserts (for
  example `benchDelta`). A gap outside it blocks the work package; it is never
  accepted here.
- **D-21 Contracts.** The files in `contracts/` as generated at the pinned commit
  are frozen. Changing one needs a new entry here and regenerated files.

## Open questions for Ritchie

- **GitLab or GitHub?** The plan puts the port on GitLab with GitLab CI; the work
  is on GitHub (D-2). Moving later is a `git push` to a new remote.
- **Datasets.** The network policy of this environment blocks
  `https://cdn.gorse.io`, where the Go model tests and the SDK test setup download
  their datasets (`PORTING_NOTES.md` O-1). Phase 3 metric parity needs either
  that host allowed or the dataset files placed in the repository's environment.
