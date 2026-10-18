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
| 1. Foundation | in progress | Done: WP-1-1, WP-1-2, WP-1-3, WP-1-5, WP-1-7, WP-1-9, WP-1-13. WP-1-11 part 1 (math32) merged. WP-1-4, WP-1-6, WP-1-8, WP-1-11, WP-1-12, WP-1-14 and WP-1-19 run in parallel threads (D-26); WP-1-10 and WP-1-15 to WP-1-18 here. All in `porting/work-packages.md` |
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

- **C-11 `floats.FromAny` belongs to its callers' work packages.** It converts the
  dynamic `any` results of external scripts and expressions
  (`logics/item_to_item.go:160`, `logics/user_to_user.go:147`,
  `worker/pipeline.go:58`), whose Rust value type is defined by WP-3-5, WP-3-6
  and WP-4-1. `FromAny` and `TestFromAny` are ported there, not in WP-1-3.
  WP-1-3's other Go tests, `floats_simd_test.go` (built only with
  `GOEXPERIMENT=simd`) and the riscv64 and loong64 test files, test kernels the
  port does not build (`PORTING_NOTES.md` X-9).

- **C-12 `common/expression` is not expr-lang; the expr-lang parser gets its own
  work package (WP-1-19).** Risk 3 and `porting/crate-selection.md` said Gorse
  uses expr-lang only through `common/expression` and the recommend filters.
  `common/expression` is the feedback type grammar
  `<feedback_type>[<operator><value>]`, one regexp and a float compare
  (`expression.go:28`), and imports no expr-lang. expr-lang (v1.17.8) is
  imported by `config/config.go`, whose `item_expr` validator accepts a field
  when `parser.Parse` does (`config.go:975`), and by
  `logics/non_personalized.go`, `logics/item_to_item.go`,
  `logics/user_to_user.go` and `cmd/gorse-bench`, which compile and run
  expressions. WP-1-15 (config) therefore needs expr-lang's parser before
  WP-3-6. New WP-1-19 ports the lexer and parser into a new crate
  `gorse-expr`, with accept/reject parity against Go; WP-1-15 depends on it,
  and WP-3-6 adds the checker, compiler and VM to the same crate instead of
  `gorse-logics/src/expr/`. The crate-selection row is reworded. Found by the
  WP-1-7 thread.
- **C-13 `time` is a hand port of Go's `time` package, not `chrono` (WP-1-9).**
  `porting/crate-selection.md` picked `chrono` 0.4.45 with Go layouts written
  on top. What Gorse needs from `time` is Go's exact text: `Time.String()` with
  the monotonic suffix `m=+…` (the cache stores it, `PORTING_NOTES.md` Q-1), Go
  layouts in both directions with Go's `ParseError` texts (dateparse builds a
  layout and calls `time.Parse`), `Duration.String` and `ParseDuration`
  (configs), `MarshalJSON`/`UnmarshalJSON`, and `LoadLocation`'s search order,
  TZif reader and errors. With those ported, `chrono` contributes nothing (it
  has no monotonic reading and reads zones differently), so it is not a
  dependency; the crate-selection row now says so. Found by the WP-1-9 thread.
- **C-14 dateparse needs `strconv.Quote` (WP-1-9).** Its errors quote the input
  with `%q`, so the new module `gorse_common::gostrconv` ports `Quote`,
  `AppendQuote`, `QuoteRune` and `IsPrint` (with Go's print table, generated).
  WP-1-14 extends it with `ParseInt`, `ParseUint` and `Atoi`; later ports that
  quote Go strings use it.
- **C-15 `chewxy/math32` is a dependency to port (WP-1-11).**
  `porting/work-packages.md` and `porting/crate-selection.md` did not list it,
  though `common/nn`, `common/ann`, `dataset` and both model packages call it
  and their results depend on its exact bits. Rust's `f32::exp` and `f32::ln`
  call the platform libm, whose results differ. It is a hand port in
  `gorse_common::math32` (v1.11.2, the version `go.mod` pins), owned by
  WP-1-11 and pushed ahead of `common/nn` because WP-1-10 needs `Log` and
  `Floor`; `crate-selection.md` gains its row. Found by the WP-1-11 thread.

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
  storage tests against `docker-compose`). Ritchie confirmed GitHub over GitLab
  on 2026-10-17. Parallel threads use their own branches (D-26).
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
- **D-22 Reviews.** The plan's definition of done asks for a reviewer agent's
  approval. A reviewer agent reviews each phase's work packages together, at the
  end of the phase and before its exit gate, against the definition of done; its
  findings are fixed or answered, and the review and the answers are recorded in
  the commit that closes the phase. One review per phase instead of one per work
  package keeps the cost proportional while every work package is still read.
- **D-23 Input Go mishandles or Gorse never writes.** Where Go panics, truncates,
  or allocates without bound on corrupt binary input, the Rust code returns an
  error (X-7), and the gob decoder rejects gob values outside the types Gorse
  writes (X-8). Every input Gorse produces reads the same in both; a crash is not
  part of the contract, and supporting types nobody writes would be untested
  code.
- **D-24 `contracts/model-blobs.md` corrections (WP-1-2).** The gob subset gains
  top-level `string` (written by `logics.MatrixFactorizationItems` and
  `encoding_test.go`), the golden table notes which type ids a fresh process
  uses, and the subset limits of the Rust decoder are stated. The byte formats
  are unchanged. Evidence: `crates/gorse-common/tests/data/gob_golden.json`,
  generated by `contracts/tools/gobgolden`.

- **D-25 Numerics of `common/floats` and the reference platform.** The SIMD
  kernels are Go's own C sources (`common/floats/src`), compiled by `build.rs`
  with clang and the options goat gave clang when it generated Go's assembly.
  The bit-exact reference for the whole port is Go on linux/amd64 as released
  (`.goreleaser.yaml`: `CGO_ENABLED=0`, default `GOAMD64=v1`), where the Go
  compiler never fuses `a*b + c` in Go code (the fusing rule in
  `cmd/compile/internal/ssa/_gen/AMD64.rules:1501` needs `GOAMD64>=3`) and Rust
  never contracts either. Evidence: `tests/floats_golden.rs` matches Go bit for
  bit at the AVX-512, AVX and scalar levels; built with gcc instead of clang it
  fails (AVX-512 `dot` of length 6), so clang is a build requirement. On arm64
  Go fuses `a*b + c` in ordinary Go code (`ARM64.rules:1798-1801`); the port does
  not mirror that outside the NEON kernels, so arm64 results can differ from Go
  arm64 in the last bit wherever Go code multiplies and adds (in `floats`, only
  `MulAddTo`). Mirroring it would mean predicting the Go compiler's fusion
  decisions expression by expression.
- **D-26 Parallel threads.** Ritchie asked why one thread did all the work
  (2026-10-17). Work packages whose dependencies are met run in parallel
  threads, each on its own `claude/wp-*` branch off
  `claude/phase-0-contracts-lqc6fr`, with the same definition of done. The main
  thread integrates: it merges each finished branch, assigns the final `C-`,
  `D-`, `Q-` and `X-` numbers to the entries the thread wrote in
  `porting/notes/<WP>.md` (provisional ids, so parallel threads never collide),
  and reruns the workspace checks after the merge.

- **D-27 `common/expression` keeps the Go pattern (WP-1-7).** The `regex`
  crate (1.13.1) shares RE2's syntax for this pattern, leftmost-first submatch
  semantics, ASCII classes and `$` matching only at the end, so the port uses
  Go's regular expression verbatim. A corpus of 902 parse cases (every
  expression in the Go tests and configs, edge cases, random strings over the
  pattern's alphabet, random well-formed values) agrees with Go on every split,
  rejection and value bit.
- **D-28 `serde` stands in for `MarshalJSON`/`UnmarshalJSON` inside larger
  documents (WP-1-7).** The Go methods are also ported as `marshal_json`
  (byte-exact, HTML-escaped like `json.Marshal`) and `unmarshal_json` (Go's
  errors); `serde` and `serde_json` become dependencies of `gorse-common`.
- **D-29 `ExprType` is a closed enum (WP-1-7).** Go's `ExprType` is an `int`, so
  Go code could build other values, but no input does: `FromString` maps to the
  five constants and `FromPB` keeps the field for unknown enum numbers
  (`PORTING_NOTES.md` Q-51). Go's `Match` is `matches` (`match` is a Rust
  keyword).
- **D-31 Go strings are bytes in `gotime` and `dateparse` (WP-1-9).** Parsers
  take `impl AsRef<[u8]>` and accept any bytes, as Go does; results Go can
  return as invalid UTF-8 are `Vec<u8>` (`dateparse::parse_format`). Errors keep
  Go's exact bytes behind accessors (`dateparse::Error::error_bytes`,
  `gotime::ParseError::value` and friends); their `Display` replaces invalid
  UTF-8 with U+FFFD.
- **D-32 `gotime` keeps Go's shape (WP-1-9).** Go's `int` is `i64`, Go's
  wrapping arithmetic wraps, `time.Date`/`time.Unix`/`time.Now` are free
  functions (`date`, `unix`, `now`), the mutable `time.Local` is
  `local()`/`set_local()`. `Location` is an `Arc`; `==` on `Location` and on
  `Time` compares like Go's `==` (pointer identity of the location),
  `Time::equal` compares instants and `Location::deep_equal` stands in for
  `reflect.DeepEqual`.
- **D-33 Tables Go computes are generated from Go (WP-1-9)**, not derived from
  Rust's Unicode data: `dateparse/tables.rs` (`unicode.IsLetter`, `IsDigit`,
  `ToLower` over every code point, Unicode 17.0.0), `gostrconv/tables.rs`
  (`strconv.IsPrint`) and `gotime/errno.rs` (the `syscall.Errno` texts
  `LoadLocation` reports), all written by `contracts/tools/timegolden`.
- **D-34 Hermetic zone data for tests (WP-1-9).** The tool writes the TZif files
  of the zones the tests use to `crates/gorse-common/tests/data/zoneinfo/` and a
  stand-in GOROOT zip (`tests/data/goroot/lib/time/zoneinfo.zip`, Go's
  uncompressed format) from go1.27.0's own `lib/time/zoneinfo.zip`. The tests
  turn the system zone directories off (`internal::disable_platform_sources`)
  and point the GOROOT fallback at the stand-in, so results do not depend on
  the machine's tzdata.
- **D-35 Time golden corpus (WP-1-9).** `gotime_golden.json`: 400 `Format` cases
  over 38 layouts and 19 `Local` zones, 2061 `Parse`/`ParseInLocation` cases,
  351 `Date` normalizations, 26 `Duration.String`, 77 `ParseDuration`, 60
  `UnmarshalJSON`/`UnmarshalText`. `dateparse_golden.json`: 17615 cases of
  `ParseAny`, `ParseIn`, `ParseLocal`, `ParseStrict` and `ParseFormat` over
  dateparse's own test inputs, edge cases, the values Go nodes write to the
  cache and a seeded corpus of mutations, in several `Local` zones and with
  each option. Every case must match exactly (instant, location name and
  offset, or error text, or layout).

## Open questions for Ritchie

- **Datasets.** The network policy of this environment blocks
  `https://cdn.gorse.io`, where the Go model tests and the SDK test setup download
  their datasets (`PORTING_NOTES.md` O-1). Phase 3 metric parity needs either
  that host allowed or the dataset files placed in the repository's environment.
