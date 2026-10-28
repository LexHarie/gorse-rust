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
| 1. Foundation | in progress | Done: WP-1-1, WP-1-2, WP-1-3, WP-1-4, WP-1-5, WP-1-6, WP-1-7, WP-1-8, WP-1-9, WP-1-10, WP-1-11, WP-1-12, WP-1-13, WP-1-14, WP-1-16, WP-1-19. WP-1-15: TOML, dotenv, config.go, YAML and the log swap merged (D-78, D-82 to D-95, X-48 closed); config.schema.json and config.keys.md not generated (X-49). WP-1-17 and WP-1-18 are in progress (WP-1-18 follows WP-1-16 and WP-1-17). All in `porting/work-packages.md` |
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
- **C-16 TLS needs a patched `rustls` (WP-1-4).** Go workers and servers dial
  the master with `util.NewClientCreds`, built on grpc's `advancedtls`, whose
  `ClientHandshake` uses the dial target `host:port` as the TLS server name
  (it does not strip the port), and Go's `crypto/tls` sends it as is. Stock
  `rustls` 0.23.46 aborts the handshake on such a name
  (`ServerNameMustContainOneHostName`), so a Rust master would refuse every Go
  client: unpatched, 34 of the 62 interop cases differ, every one that reaches
  a handshake. Ritchie chose the patch on 2026-10-18 (decision card "How should
  a Rust master accept Go workers' TLS connections?"): `third_party/rustls` is
  the published 0.23.46 with one change in `src/server/hs.rs`, so the server
  ignores a server name that is not a DNS name, as it already did for an IP
  address. It is wired in with `[patch.crates-io]` and kept out of the
  workspace with `exclude`; `third_party/rustls/GORSE.md` says how to upgrade.
  `third_party/`, not `vendor/`, because the Go toolchain gives a `vendor/`
  directory next to `go.mod` its own meaning. `porting/crate-selection.md`
  gains a TLS row.
- **C-17 `aws-lc-rs`, not `ring` (WP-1-4).** The certificates Gorse's own
  TestSSL makes (`madflojo/testcerts`) are P-521, which `ring` cannot verify.
- **C-18 WP-1-14 depends on WP-1-9 (WP-1-14).** `work-packages.md` lists no
  dependency. However, `Options.ConnMaxLifetime` is a `time.Duration`, and
  the MySQL DSN's `loc` parameter goes through `time.LoadLocation`, its
  timeouts through `time.ParseDuration`. The port uses `gorse_common::gotime`.
- **C-19 `scheme.go` needs Go standard library ports that the plan does
  not list (WP-1-14).** Gorse's results and errors depend on their exact behaviour:
  - `net/url` (`AppendURLParams`, and every store that parses a URL). This
    is `gourl`, with `ParseRequestURI`, `Values`, the escape functions, and
    the `urlstrictcolons` and `urlmaxqueryparams` GODEBUG settings, which it
    reads as Go does.
  - `strings.Replacer` (gorm's names, and later the Redis and xvec stores).
    This is `gostrings`.
  - `encoding/json` into `any`, `[]any` or `map[string]any`
    (`json_contains`, labels, viper's config codec for WP-1-15). This is
    `gojson`.
  - `unicode/utf8`. This is `goutf8`.

  They live in `gorse-common` so that later packages share them. Each has a
  `Ports:` header that names the go1.27.0 files.
- **C-20 `scheme.go` needs parts of two third-party libraries (WP-1-14).**
  - `AppendMySQLParams` round-trips the DSN through go-sql-driver/mysql
    v1.9.3's `ParseDSN` and `FormatDSN`. That is `mysql_dsn.rs` (MPL-2.0, a
    file-level licence; the licence text is in
    `crates/gorse-storage-core/licenses/`).
  - `NewGORMConfig`'s `NamingStrategy` decides every table, column, index
    and constraint name that a Go node creates. That is gorm v1.31.1's
    `schema/naming.go` in `gorm_naming.rs` (MIT, licence alongside).
  - `crypto/sha1` (gorm's `formatName`) adds a dependency, `sha1` 0.11.0,
    with a row in `porting/crate-selection.md`.
- **C-21 Go 1.27's `encoding/json` is the v2 decoder (WP-1-14).** In go1.27,
  `GOEXPERIMENT=jsonv2` is on by default, so `json.Unmarshal` is v1's API
  over v2's implementation. The order of operations changes, and so do some
  texts:
  - The whole input is checked for syntax first; then the top-level kind
    ("json: cannot unmarshal array into Go value of type
    map[string]interface {}"); then the value is decoded.
  - A float overflow names its path ("json: cannot unmarshal number 1e400
    into Go struct field .master.port of type float64").
  - A duplicate name replaces the earlier value whole: `{"a":{"b":1},"a":{"c":2}}`
    decodes to `{a:{c:2}}`.

  `serde_json` differs on each of these points, so `gojson` is a hand port,
  with objects in an `IndexMap` (`indexmap` 2.14.2, new row in
  `crate-selection.md`).
- **C-22 The driver glue is Phase 2 work (WP-1-14).** Two pieces need a live
  connection, which WP-1-14 has none of:
  - registering `json_contains` on SQLite connections (the `rusqlite` choice,
    D-18);
  - `ProbeMySQLIsolationVariableName`'s query.

  WP-1-14 ports the logic. The SQL stores (WP-2-2, WP-2-6 and, for SQLite,
  WP-2-11) wire it up, with the obligations in O-6.

- **C-23** `porting/crate-selection.md` names the `toml` crate (1.1.8)
  for viper's TOML. The port does not use it: go-toml's accepted syntax,
  error texts and positions, `\e`, `int64`/`LocalDate` types and the quirks
  above differ from any other TOML parser, and viper's errors reach the user
  (`config.LoadConfig` returns them, `config/config.go:826`). That row is not
  edited here (not this part's file).

- **C-24** `porting/work-packages.md` lists only Gorse's
  `config/config.go` for WP-1-15; part T ports third-party code (go-toml
  v2.2.4) under `crates/gorse-config/src/formats/toml/`.

- **C-25 gotenv's `TestScanner` does not pass on go1.27.0.** Three of its
  seven cases expect a final empty line after a trailing terminator
  (`"aa\nbb\ncc\n"` → `aa, bb, cc, ""`). Since Go 1.22, `Scanner.Scan` returns
  `token != nil` for `ErrFinalToken`, and `splitLines` returns a nil token at
  EOF, so Go 1.27 gives three lines. Evidence: gotenv v1.6.0's tests run with
  `$GOROOT/bin/go test` (go1.27.0, x/text v0.36.0, testify v1.11.1 from the
  module cache, in a scratch copy): `--- FAIL: TestScanner … expected: 4
  actual: 3` for "regular LF/CR/CRLF split with trailing …"; every other gotenv
  test passes. The port (`scan.rs`, `gotenv_test_scanner`) asserts Go 1.27's
  lines, Q-125.



- **C-26 The `%v` float printers of WP-1-7 and WP-1-19 must settle ties as Go does
  (WP-1-15 config drafts).** `gorse-common/src/expression.rs` (WP-1-7) and
  `gorse-expr/src/gostr.rs` (WP-1-19) took Rust's shortest digits as they are, so a
  feedback type expression or an expr-lang float literal at a shortest-digit tie
  printed differently from Go. Both now call `gostrconv::format_float(v, b'g', -1, 64)`;
  `gorse-expr` depends on `gorse-common` for it. `expression_golden.json` gained 2^-25
  (format cases) and its decimal form (parse cases); the old code fails them. Found
  by the config-drafts thread's ftoa work.

- **C-27 juju/ratelimit is its own LGPL crate.** juju/ratelimit is LGPLv3
  with a static-linking exception, and a port is a modified version of it, so
  the plan's "hand port (~100 lines)" inside the Apache-2.0 `gorse-common`
  would have put LGPL code in an Apache crate. Ritchie chose a separate crate
  on 2026-10-19 (decision card "Choose how to port the LGPL-licensed rate
  limiter Gorse uses for OpenAI calls", option "Own LGPL crate"; the
  integrator's later suggestion of a clean-room Apache rewrite came after the
  card was answered). `crates/gorse-ratelimit` carries juju's `LICENSE`
  verbatim (exception and LGPLv3 text), the GPLv3 text the LGPL supplements
  as `COPYING` (identical to the GNU project's own copy: sha256
  `3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986`, the
  same as automake's `COPYING`), and a dated modification notice in
  `README.md` and every source header (LGPLv3 section 2 via GPLv3 section
  5a). Its `license` is
  `(LGPL-3.0-only WITH LGPL-3.0-linking-exception) AND BSD-3-Clause`: the
  SPDX exception `LGPL-3.0-linking-exception` is juju's text word for word
  (SPDX lists juju/errors as a source), and the BSD part is
  `src/format.rs`, a few lines of Go's `strconv` (with `licenses/GO-LICENSE`).
  `porting/crate-selection.md`'s row is updated.

- **C-28 Nothing calls `Take` in add21b5.** The crate-selection row said
  "exact refill semantics matter for the 429 behaviour". The only use of
  `ratelimit.go` is `master/master.go:137-138`, which calls the two init
  functions; no code reads the limiters or the backoffs. What a Rust master
  must reproduce today is the start-up panic for a limit of 1 to 59 per
  minute (Q-152) and the arithmetic; the whole bucket is ported anyway
  (juju's tests, and future callers) and checked against Go.

- **C-29 A Go `context` port.** The schedulers take a `context.Context`;
  the plan had no row for it. `gorse_common::gocontext` ports the
  cancellation tree; `porting/crate-selection.md` gains a row
  (`tokio-util`'s `CancellationToken` is async only, and the schedulers'
  callers block on the context from plain threads).

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
- **D-30 The gob subset gains `[]float32` (WP-1-10).** `ann.HNSW.Marshal`
  writes each vector with `WriteGob`, so a `logics.MatrixFactorizationItems`
  blob holds top-level `[]float32` values. Only `logics/cf_test.go` creates
  one, so no Gorse process reads one written by another; the codec covers the
  type so that HNSW blobs read Go's bytes and write them back unchanged.
  Evidence: three `[]float32` encodings and nine decodes (errors included) in
  `crates/gorse-common/tests/data/gob_golden.json` and
  `gob_decode_golden.json`, and the HNSW blobs of `ann_golden.json`.
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
- **D-36 The input is `&str` (WP-1-19).** A Go string may hold any bytes; Gorse's
  `item_expr` values come from TOML (UTF-8 by its spec) and the
  `RECOMMEND_CONFIG` JSON override in the meta store (`master.go:164`, decoded
  to UTF-8); no environment binding sets one (`config.go:742-797`). So text is
  enough. The corpus inputs that are not UTF-8 cannot be passed and are counted per source
  in the golden files (two: one row of `TestLex`, one string literal of
  another expr-lang test).
- **D-37 Token values and `StringNode` values are bytes (`Vec<u8>`) (WP-1-19).**
  Go strings are bytes, and an escape can make a string literal's value
  invalid UTF-8 (`PORTING_NOTES.md` Q-85). `Token::value_str` gives the text where it is
  UTF-8; identifiers and operators always are.
- **D-38 `parse` returns `Result<Tree, Error>` (WP-1-19).** Go returns the
  half-built tree together with the error; every caller (Gorse's validator,
  `expr.Compile`, `expr.Eval`) drops it, so here the error is all there is.
  The parser keeps Go's control flow after an error exactly (it decides which
  error is reported, Q-86); a `None` child always comes with an error,
  so no node with a missing child is ever built.
- **D-39 Shared subtrees are cloned (WP-1-19).** Go points the condition and the
  first branch of `a ?: b` at one node, and neighbouring comparisons of a
  chain `a < b < c` at one middle node (`PORTING_NOTES.md` Q-95). A Rust tree owns its
  nodes, so these are equal copies. For WP-3-6: Go's checker and patchers
  visit such a node twice through the same pointer; with copies each is
  visited once, which gives the same result as long as visiting is
  deterministic.
- **D-40 Any depth, like Go (WP-1-19).** Go's stacks grow to 1 GB, and a config
  may turn the node budget off (`MaxNodes: 0`), so depth is unbounded. The
  recursive functions (parser, printer, dump, walk, clone, compare, debug)
  run under `stacker::maybe_grow` (`stacker` 0.1.25, recorded in
  `porting/crate-selection.md`), and dropping a tree iterates.
  `tests/parser_deep.rs` parses, prints, walks, clones and compares trees
  100,000 levels deep on a 256 KiB thread; the synthetic goldens include
  200,000 nested parentheses and node chains on both sides of the budget.
- **D-41 What waits for WP-3-6 (WP-1-19).** `ConstantNode` (only the optimizer
  makes it; `TestPrint_ConstantNode` is not ported), the `nature`/type of each
  node and the `Node` interface's type methods, the rest of `conf.Config`
  (environment types, function values, builtins table, optimizer and VM
  switches), `file.Error.Prev` and the JSON tags of `file.Error`. The parser
  needs of `conf.Config` only `MaxNodes`, `Disabled`, `DisableIfOperator` and
  `IsOverridden`; the Rust `Config` holds the names of the functions and of
  the environment's entries as sets. Go's `IsOverridden` asks
  `Env.Get(name)` of the environment's nature; WP-3-6 derives the set from
  the environment's type.
- **D-42 The API is `parser::parse`, `parser::parse_with_config`,
  `parser::lexer::lex` and `Lexer::new(&Source)` (WP-1-19).** Go's reusable `Parser`
  (with a `Parse` method) and `Lexer.Reset` are not ported (`PORTING_NOTES.md` Q-93); one
  lexer per source.
- **D-43 Character classes come from Go's tables (WP-1-19).** `unicode.IsLetter`,
  `IsDigit`, `IsSpace` and `strconv.IsPrint` are generated from Go 1.27.0
  (Unicode 17.0.0) into `unicode_tables.rs`, so the lexer and the quoting of
  error messages follow Go's Unicode version rather than Rust's. A unit test
  compares all four with Go for every code point.
- **D-44 The Go test tables are read from the Go source (WP-1-19).**
  `exprparsegolden` (tables.go) turns the tables of `TestParse`,
  `TestParse_error`, `TestParse_optional_chaining`, `TestNodeBudget`,
  `TestLex` and `TestPrint` into `parser_go_tests.json` (expected trees as
  JSON, unset fields left to Go's zero values), and
  `tests/parser_go_tests.rs` runs them with Go's assertions (`Dump(want) ==
  Dump(actual)`, the error text, `compareTokens`, `Node.String()`).
  Hand-ported: `TestParse_pipe_operator`, `TestNodeBudgetDisabled`,
  `TestLex_location`, `TestLex_error` (its table is a constant, split as Go
  splits it), `TestPrint_MemberNode`, `ast` `TestWalk`, `TestWalk_patch`,
  `TestFind`, and `file` `TestStringSource_Snippet*`.
- **D-45 Licence (WP-1-19).** The crate ports MIT-licensed code: `license =
  "Apache-2.0 AND MIT"`, with expr-lang's licence in
  `crates/gorse-expr/LICENSE-expr-lang`.
- **D-46 Go's checks on the CA certificate (WP-1-4).** `advancedtls` in
  `CertVerification` mode runs `x509.Verify` with the purpose's extended key
  usage and no host name. `webpki` keeps only the name and key of a trust
  anchor, so `util/tls.rs` runs Go's other checks in `webpki`'s path callback,
  on every chain `webpki` builds (a rejected chain makes it try the next, as Go
  tries other chains): the CA must be valid now, its extended key usage (if
  any) must include the purpose or `anyExtendedKeyUsage`, the number of
  intermediates must respect its `MaxPathLen`, and every issuer, the CA
  included, must pass Go's `CheckSignatureFrom` constraints (a v3 certificate
  needs `BasicConstraints` with `cA`, and a `KeyUsage` with any bit set must
  include `keyCertSign`). A custom `ExtendedKeyUsageValidator` accepts
  `anyExtendedKeyUsage` on the leaf, as Go does and `webpki` does not.
  `x509-parser` reads the fields.
- **D-47 Verdicts, not error texts, for TLS handshakes (WP-1-4).**
  `tls_golden.json` records Go's error texts for reading; the tests compare
  whether each side built its credentials, whether the handshake and a
  ping/pong went through, which side rejected the other, and the ALPN
  protocol (`PORTING_NOTES.md` X-37). Credential errors that Gorse surfaces at
  start-up match Go's text: `open <path>: <os error>`, `failed to append
  certificate`, `tls: failed to find any PEM data in certificate input` /
  `key input`, and `tls: ...` for a key that does not match its certificate.
- **D-48 Number parsing lives in `gostrconv` (WP-1-4).** WP-1-4 first ported
  `Atoi`, `ParseInt`, `ParseUint` and `ParseFloat` under `util::strconv`, then
  dropped its integer copies for WP-1-14's and added only `ParseFloat` as
  `gostrconv/atof.rs`, in the same shape (0 with a syntax error, the signed
  infinity with a range error). `util_golden.json` also checks `gostrconv`'s
  `atoi`, `parse_int` and `parse_uint` with bit size 0, including the value
  Go returns next to an error, and `quote` and `is_print` at every edge of
  Go's print table: all match. WP-1-14 takes over `atof.rs` (D-52).
- **D-49 `util::parse_float`, `parse_int` and `parse_uint` return a `Result`
  (WP-1-4).** Go's `util.ParseFloat[T]` and friends also return a value next to
  an error; every Gorse caller (`common/datautil/datautil.go`,
  `storage/cache/redis.go`, `dataset/dataset.go`) returns the error without
  reading it. Inputs are bytes (`impl AsRef<[u8]>`), as in `gostrconv`.
- **D-50 The rand tables come from Go (WP-1-4).** `util/rand/tables.rs`
  (`rngCooked`, the ziggurat tables of `normal.go` and `exp.go`) is written by
  `randgolden` from Go's own values. The decimal constants of Go's amd64
  `math.Exp` and `math.Log` are kept as Go's text, and a test checks the bits
  Go's assembler gives them.
- **D-51 Go's top-level `rand` functions are not ported (WP-1-4).** Since Go
  1.20 they draw from a randomly seeded runtime source and `rand.Seed` is a
  no-op, so nothing about them is reproducible.
- **D-52 One owner for `ParseFloat` (integrator, 2026-10-19).** WP-1-4's
  `atof.rs` ports Go's syntax (`special`, `readFloat`, `atofHex`) and hands the
  decimal digits to Rust's correctly rounded parser. go1.27 rounds correctly
  too, except where its slow path keeps only 800 digits (`decimal.set`), which
  WP-1-14's golden reaches: `"9007199254740993" + "0"*790 + "1e-791"` is
  `41cad7f29abcaf49` in Go and `4340000000000001` with the Rust parser. WP-1-14
  owns `atof.rs` from here and replaces it with its structural port of go1.27
  (`atof64exact`, `atof32exact`, `uscale.go`, `decimal.go` with the 800-digit
  cap), checked against WP-1-4's golden, its own and the 800-digit cases, and
  adds `ParseBool` (`atob.go`).
- **D-53 A `json_contains` element that Go cannot hash is an error, not a
  panic (WP-1-14).** In Go, an array or object element panics inside the `map[any]`
  insert or lookup, and the panic leaves the connection locked. Here it is
  `json_contains::Error::Unhashable`, carrying the text Go's panic prints
  (Q-107).
- **D-54 `gojson` objects keep document order (WP-1-14).** Go's maps have no order,
  so any order matches Go. Document order makes iteration deterministic. A
  duplicate name keeps its first position and takes its last value.
- **D-55 Only `gojson`'s error texts are ported (WP-1-14).** Go also returns a
  partial value along with an error, and the error carries byte offsets.
  Neither is ported, because only the texts reach Gorse.
- **D-56 Go strings are bytes (WP-1-14).** URLs, DSNs, query keys and values, table
  names and gorm names are `Vec<u8>` / `impl AsRef<[u8]>`. Gorm's mixed
  byte and rune indexing can produce invalid UTF-8 exactly where Go's does.
  `DsnError`'s `Display` shows U+FFFD, and `DsnError::error_bytes` gives
  Go's bytes.
- **D-57 A non-UTF-8 MySQL `loc` name (WP-1-14).** `LoadLocation` takes a `&str`.
  A name with `..`, a leading `/` or a leading `\` is `InvalidName`, as in
  Go. Any other non-UTF-8 name is `UnknownTimeZone`: zone files have ASCII
  names, so Go finds none, and the golden cases confirm it.
- **D-58 `probe_mysql_isolation_variable_name` takes the query as a
  closure (WP-1-14).** Go opens a connection with the driver. Here the caller passes a
  function that runs the query on the parsed `Config`. The DSN is parsed
  first, as Go's `sql.Open` (`OpenConnector`) does, so a DSN error comes
  before any connection.
- **D-59 Only gorm's singular names are ported (WP-1-14).** `NewGORMConfig` always
  sets `SingularTable: true`, so `jinzhu/inflection`'s plurals are not
  ported, and neither are `SchemaName` or `RelationshipFKName` (Gorse has no
  relationships). `GormConfig` has no logger: Go sets a silent one.
- **D-60 Only the DSN side of go-sql-driver/mysql is ported (WP-1-14).** The
  connection side (`Logger`, `DialFunc`, `BeforeConnect`, `Config.Apply`) is
  not. A failed `parse_dsn` returns no partial `Config`. A `strict`
  parameter panics with Go's message, as in v1.9.3.
- **D-61 `Opt` is a boxed `Fn(&mut Options)` (WP-1-14)** (Go's `func(*Options)`).
  The counts are `i64`, because Go's `int` is 64 bits on every platform
  Gorse ships for. `apply_sql_pool` takes a `SqlPool` trait standing in for
  `*sql.DB`.

- **D-62 Only the `map[string]interface{}` path of the decoder.** The
  reflect-based decoder is ported as `Target::Interface` (an `interface{}`
  slot: a map entry, a slice element) and `Target::Concrete` (a value of
  concrete type: `map[string]interface{}`, `[]interface{}`), which is all a
  map root reaches. Not ported: structs, pointers, typed maps and slices,
  arrays, `encoding.TextUnmarshaler`, `unstable.Unmarshaler`, strict mode
  (`DisallowUnknownFields`, `internal/tracker/key.go`, `DecodeError.Key`,
  which only strict mode sets: `errors.go:168` from `strict.go:58`, `:70`),
  `KeepComments` comment nodes, `Parser.Shape`, the `LocalDate`/`LocalTime`/
  `LocalDateTime` methods (`AsTime`, `String`, `MarshalText`,
  `UnmarshalText`), `Decoder`/`NewDecoder` (viper calls `Unmarshal`), and
  the marshaler.

- **D-63 Unreachable branches stay as panics.** Where go-toml panics on
  an impossible state (`decode.go:22` invalid base, `:93` missing timezone,
  `seen.go:170` node kind, `unmarshaler.go:299`, `:554`, `:707`) or would hit
  a reflect panic on a path a map root cannot take, the port panics with
  Go's text. The golden records Go panics and has none.

- **D-64 Strings are UTF-8.** Keys and string values become Rust
  `String`s with `from_utf8_lossy`. go-toml validates every key and string as
  UTF-8 before it stores it, so nothing is ever replaced: the
  golden writes non-UTF-8 Go strings as hex and has none (10,970 cases,
  7,110 of them with random bytes).

- **D-65 Recursion on a growing stack.** `parse_val`, the tracker's
  `check_array`, `check_inline_table`, `set_explicit_flag` and `clear`, and
  the decoder's `handle_value`, `handle_key_part` and
  `handle_key_value_part` run inside `stacker::maybe_grow` (128 KiB red
  zone, 4 MiB segments), as `gorse-expr` does; the parser keeps Go's
  recursive structure. On an error, the partly decoded map is dropped
  iteratively (`drop_deep`). The golden test runs on the default test
  stack, 10,000-level documents included. `stacker` (already a workspace
  dependency) is added to `gorse-config`; its `crate-selection.md` row says
  so.

- **D-66 The input is taken to have spare capacity** (Q-123),
  as viper's always does; `unmarshal`'s doc says so.

- **D-67 Keys keep document order.** `Map` is ordered by first
  insertion; Go's map order is random, so nothing can depend on it.

- **D-68 Golden encoding.** Each map is a flat pre-order list of nodes
  (`k`, Go type name `t`, value `v`, child count `n`), so neither Go's
  `encoding/json` nor `serde_json` meets its nesting limit on 10,000-level
  documents; floats are their 16 hex digits of `math.Float64bits`; a
  `time.Time` has its zone name, offset and location name; non-UTF-8 text is
  hex.

- **D-69 Go strings are bytes.** The parser works on `&[u8]` end to end.
  `strict_parse` returns gotenv's `(Env, error)`: an `Env` of byte strings in
  the order keys were first set (a `HashMap` index for lookups) and the
  error's text as bytes; the variables set before an error come back with it,
  which is what `gotenv.Parse` returns and what the ported `TestParse` checks.
  `decode` is the codec: it drops them on error, as viper does, and turns the
  bytes into Rust strings (X-40). Keys always match `[0-9A-Za-z_.]+`, so
  they are ASCII and converting them never merges two keys.

- **D-70 The read sizes are ported, not just the line rules.** Where a
  line ends depends on what the scanner has buffered (Q-126), so the
  scanner keeps Go's buffer (nil, then 4096 doubling to 65536, shifting when
  `start > 0` and the buffer is full or more than half consumed) and every
  reader in front of it returns what Go's returns: `bytes.Buffer.Read` copies
  `min(len(p), remaining)`, the probe's ≤3 bytes come back alone on the first
  read, `transform.Reader` has 4096-byte source and destination buffers.

- **D-71 The patterns are Go's, spelled out for the regex crate.** Go's
  `\s` is `[\t\n\f\r ]` and `\w` is `[0-9A-Za-z_]`; the regex crate's are
  Unicode, and its ASCII `\s` includes `\v`, which Go's does not. So the Rust
  patterns write the classes out, run on bytes with Unicode off (`(?-u)`), and
  `\'`, `\"`, `\#` are plain characters. In Go, `.` and the negated classes
  match any rune, with each invalid byte read as U+FFFD; every class in these
  patterns either contains all non-ASCII runes or none, so matching bytes gives
  the same spans. Both engines choose the leftmost-first match and its groups
  the way a backtracking engine would. A unit test derives the Rust
  `linePattern` from the Go text by those substitutions
  (`go_patterns_spelled_out`), and the golden checks the behaviour.

- **D-72 Only `StrictParse`'s path is ported.** `override` is always false
  (`Load`, `OverLoad`, `Apply`, `OverApply` set process variables; viper never
  calls them), so `varReplacement` reads the environment first. Its `hsq`
  parameter is kept although the caller always passes false, as in Go.
  `Marshal`, `Write` and viper's `Encode` are not part of decoding.

- **D-73 Guards that cannot fire are left out.** The probe's
  `err != io.EOF` check (a `bytes.Buffer` cannot fail) and the scanner's
  `ErrBadReadCount`, `ErrNegativeAdvance`, `ErrAdvanceTooFar` and its panic on
  100 empty tokens (the readers never return more than asked; `splitLines`
  always advances when it returns a token). `ErrNoProgress` and every
  `transform.Reader` error branch are ported, with Go's texts, although the
  decoders here never reach them (the BOM is always there when a UTF-16
  decoder runs, and no sequence needs more than 4 bytes of look-ahead).

- **D-74 `env` is `os.LookupEnv`.** `decode` and `strict_parse` take
  `&dyn Fn(&str) -> Option<String>`; tests pass the case's environment. Names
  are `[A-Z0-9_]+`, so they are ASCII.

- **D-75 Golden format and corpus.** One case per line; each names an
  environment from the file's `environments` (variables set and unset; the
  tool clears the environment, so nothing else is visible). A byte string is
  a JSON string when it is valid UTF-8 and at most 512 bytes, `{"text", "hex"}`
  when it is not UTF-8, and `{"len", "rle"}` (hex units repeated) when longer.
  Every error is also checked against `viper.ReadConfig`, which must say
  `While parsing config: ` and the same text. The corpus: gotenv's 42
  `formats`, 2 `errorFormats`, `TestMarshal`'s text and its 10 files (57);
  the 9 dotenv files of `config_golden.json`, with `HOME` set and unset (18);
  201 branch inputs with and without an environment (402); 2728 generated
  lines (every value with every separator, every prefix with every key, 1500
  random lines and 600 random files); 69 byte order mark cases; 27 invalid
  UTF-8 inputs; 1286 boundary cases (a break at line offset `2^k·4096 − 1 + δ`
  for δ in −3..2 after five different prefixes, through the plain reader and
  through x/text for UTF-8+BOM, UTF-16LE and UTF-16BE at multiples of 2048,
  wide characters and invalid UTF-8 that overflow x/text's buffers, and 60
  files of random line lengths); 206 lines of 4094 to 70000 bytes; 4000
  seeded mutations (insert, delete, replace; 92% of the written bytes from the
  characters gotenv cares about). 3684 cases parse, 5109 fail.

- **D-76 gotenv's files.** `crates/gorse-config/tests/data/gotenv/` holds
  copies of gotenv v1.6.0's `.env`, `.env.invalid` and `fixtures/*` for the
  ported tests (`diff -r` against the module: identical). The golden tool
  embeds them as Go literals and `-gotenv-dir` checks them byte for byte.

- **D-77 Dependencies.** `regex` (already a workspace dependency, row
  "regexp (RE2)" in `porting/crate-selection.md`) for gorse-config;
  `serde_json` and `hex` (workspace dependencies) as dev-dependencies for the
  golden test. No new crate.

- **D-78 Integrator handover; WP-1-15 split (integrator, 2026-10-28).** The
  integrator thread was replaced. Its WP-1-15 work sat on origin as
  `claude/wp-1-15-config` (WIP viper and mapstructure drafts), `-toml` (done),
  `-dotenv` (done) and `-yaml` (WIP, unfinished). The TOML part (`e8862f65`) and
  the dotenv part (`d4cd2518`) merged into the integration branch. Their notes were
  folded in as C-23 to C-25, D-62 to D-77, Q-111 to Q-137, X-38 to X-40 and
  O-8 to O-12, and their deviation table closed X-39 and O-9 (D-52 already
  reproduces Go's 800-digit decimal, so the four golden cases now match Go).
  The YAML part continues in its own thread from `e5f9b7d`. The config core
  (`dc3ffb4`: viper layering, mapstructure, validation, hashes) stays with the
  integrator and merges after the YAML part. Evidence: `cargo test -p gorse-config`
  passes with TOML and dotenv on the integration branch (29 unit, 3 dotenv
  golden, 11 gotenv, 6 TOML deep (1 ignored), 1 TOML golden with 10,970 cases,
  24 TOML unit), `cargo clippy -p gorse-config --all-targets -- -D warnings` and
  `cargo fmt --all --check` pass.

- **D-79 Operand order of `+` and `*` on NaN (WP-1-3 `floats.rs`).** Go
  computes `dst[i] += c` as `ADDSS X0, X1`, with the destination element first
  (`go tool objdump` of the golden tool, `Feature.addConst`, `floats.go:126`).
  `Feature.mulConstAdd` (`floats.go:156`) computes `(a[i] * c) + dst[i]` as
  `MULSS X0, X1` followed by `ADDSS 0(SI)(AX*4), X1`. When both operands are
  NaN, x86 SSE returns the NaN of the first source operand (Intel SDM vol. 1,
  table 4-7), and LLVM may swap the operands of `+` and `*`. The change adds
  `add_first`, `mul_first` and `first_nan` (`floats.rs`). Each operation the
  Go code performs with the destination first now goes through them: the native
  kernels (`add`, `add_to`, `add_const`, `mul_to`, `mul_const_add`,
  `mul_add_to`, `dot`, `euclidean`, the scalar `mm`) and the public `add` and
  `mul_add_to`. The release golden is the test (see Evidence).

- **D-80 `Exp` in `nn::deps`.** `deps::exp` calls WP-1-4's
  `util::rand::math::exp_with`, with the FMA choice from `Feature::current()`,
  which honours `GODEBUG` as Go's `internal/cpu` does. `util::rand::math` itself
  was a copy here until this part; `util::rand` now exposes `math` to the crate
  (`pub(crate) mod math`). See O-14.

- **D-81 Go's top-level `rand` in `nn::deps::global`.** Used where Go calls
  `rand.NormFloat64` and `rand.Perm` without a generator (`TestSaveAndLoad`'s
  spiral, and `rand(None, ..)` and `normal_init(None, ..)`). Go seeds that source
  randomly, so no test depends on its values, and nothing else uses it.

- **D-82 Validate is explicit ordered checks.** The Go validator tags
  (`gte=0`, `gt=0`, `oneof=`, and the `item_expr` and store checks) are coded
  as functions in `config/validate.rs`, run in struct field order, and the first
  failure wins. The messages match the golden file
  (`test_validate_messages_match_the_golden`, 36 validate cases). The
  go-playground validator is not a dependency.

- **D-83 Injected environment.** `load_config_with_env(path, lookup)` takes the
  environment lookup. `load_config(path)` reads the process environment. The
  crate forbids `unsafe`, and edition 2024 makes `std::env::set_var` unsafe, so
  the tests do not mutate the environment.

- **D-84 Logging is a placeholder** (`src/log.rs`, `TODO(WP-1-8)`). Go's
  `LoadConfig` warns when the file is missing, and the port does not print that
  warning yet. The util thread's API is `tracing` directly:
  `tracing::warn!("message")`, with no wrapper. Swap the placeholder when WP-1-8
  merges (X-48).

- **D-85 `go_json_quote` is public** in `gorse-common/src/expression.rs`. It was
  private. Config's JSON writer needs Go's string escaping.

- **D-86 `md-5` and `hex` are direct dependencies.** `gorse_common::util::md5`
  sorts its input, but Go's `md5.New()` hashes stream the parts in order
  (`NonPersonalized`, `External`, `ItemToItem`, `UserToUser` and `Collaborative`
  hashes). `md5_of_sorted` uses `util::md5` only for `RecommendConfig::hash`.

- **D-87 `serde_json` is a normal dependency** (it was a dev-dependency). It
  parses `Meta.config` in `config/json.rs`.

- **D-88 `TracingConfig::equal` takes `&TracingConfig`.** Go's nil receiver
  returns false, and a Rust reference cannot be nil.

- **D-89 JSON decode stops at the first type mismatch.** Go's `json.Unmarshal`
  records the first mismatch and keeps decoding, so fields after it are still
  set in Go. The port returns the first mismatch at once. No golden case reaches
  this, and the inputs Gorse decodes are its own `json.Marshal` output.

- **D-90 The TOML `TestUnmarshal` case runs from the text.** Go re-encodes the
  text through `yj` before decoding. The port decodes the text directly. The
  result is the same.

- **D-91** (replaced the adapter): the WIP adapter over yaml-rust2
  (e5f9b7d) is gone. The port follows yaml.v3's pipeline stage by stage:
  readerc.go, scannerc.go, parserc.go, the composer, decode.go and resolve.go.
  The adapter gave 518 golden mismatches (error texts, marks, the depth-255
  limit that yaml.v3 does not have). yaml-rust2 is not in the workspace or the
  lock. No crate-selection row named it, so none needs removing.

- **D-92** (value model, accepted by the integrator): `decode` returns
  `gorse_config::value::Map`, the model the TOML and dotenv decoders and the
  config loader use. `gojson` stays JSON-only. The coordinator's earlier
  `gorse_common::gojson::Value` note was a guess, not a requirement.

- **D-93** (deep nesting, accepted): the conversion from decoded Go values to
  `Value` (`formats/yaml.rs`, `value()`) grows the stack with
  `stacker::maybe_grow`, as the composer does. Without it, a 9990-deep document
  overflowed a 2 MiB thread. stacker is the crate-selection row's choice
  (0.1.25, latest). The row now says the YAML path uses it too.

- **D-94** (test thread, accepted): `golden_cases` replays on a 512 MiB
  thread. Depth-10000 cases overflow a default test thread in debug builds.

- **D-95** (iterative drop): `impl Drop for Value` (`value.rs`) moves each
  container's children onto one stack and drops them in a loop, so dropping a
  deep value does not recurse. The TOML decoder's `drop_deep` helper is gone,
  since the `Drop` does its job for every caller. The two test-only helpers of
  the same name (`toml_deep.rs`, `toml_golden.rs`) are removed, and their call
  sites call `drop`. Matching a `Value` by value no longer compiles, so those
  matches borrow instead (`toml.rs` `unmarshal`, `viper.rs` `insensitivise_val`,
  `dotenv_golden.rs`); behaviour is unchanged.

- **D-96 Scoped threads and a job counter.** Go's `Parallel`, `For` and
  `ForEach` start `nWorkers` goroutines reading job ids from a 1024-slot
  channel filled by a producer goroutine. Here `nWorkers` scoped threads
  take the next id from an atomic counter: jobs start in the same order, and
  a worker takes a job only when it is free, as with the channel. Go's
  producer can leak (when every worker quits early with more than 1024 jobs
  left, it blocks on the channel forever); there is no producer here.

- **D-97 The context is checked first.** Where Go `select`s between
  `ctx.Done()` and a job or a semaphore slot, it picks at random when both
  are ready. The port always takes the context, one of Go's outcomes: no
  job starts once the context is done.

- **D-98 Panics.** As in Go, `Parallel` with more than one worker
  recovers a worker's panic (`util.CheckPanic`: logs `panic recovered`, the
  worker stops, the others go on), and the sequential paths (one worker)
  let it reach the caller. In Go a panic in a worker goroutine of `For`,
  `ForEach` or `Detachable` is not recovered and crashes the process; a
  library cannot do that, so the other workers stop taking jobs and the
  first panic resumes on the caller after they return. `Detachable` gives
  back the panicking job's slot first, so its scheduler cannot hang on it.

- **D-99 A thread per `Detachable` job.** Go starts a goroutine per job.
  A job waiting in `Attach` holds no slot, so the number of live jobs is not
  bounded by `nWorkers + nMaxDetached`, and a fixed pool could deadlock
  where Go does not. Each job is a scoped thread (a spawn costs some
  microseconds; the worker pipeline's jobs take milliseconds).

- **D-100 Shapes.** Counts are `usize` (Go's `int`; negative counts cannot
  be passed). `ForEach` lends the worker `&T` where Go copies the element.
  `For` is `for_` (`for` is a keyword). `parallel::Context`'s methods take
  `&mut self`; Go's nil-receiver no-op becomes an `Option<&mut Context>` at
  the call site. The limiter package variables are behind locks, read with
  getters (`chat_completion_backoff()` ...) and set only by the init
  functions; `RateLimiter: Any`, so a caller can type-switch as Go does.

- **D-101 `gocontext` scope.** `Background`, `WithCancel`, `CancelFunc`,
  `Err`, `Done` (as `is_done`, `wait`, `wait_timeout`). A child registers a
  hook on its parent and removes it when cancelled itself (Go's
  `children` map and `removeChild`); hooks run after the parent's lock is
  released but before its cancel returns, so a cancel has reached every
  descendant when it returns, as in Go. Parents hold children strongly and
  children hold parents weakly, so dropped contexts are freed. A crate-private
  hook (`on_done`) wakes `Detachable`'s scheduler, which Go does with a
  `select`. Dropping a `CancelFunc` cancels nothing, as in Go.

- **D-102 juju's types.** Go's `time.Duration` is `i64` nanoseconds and
  `time.Time` is `Instant`; `int64` arithmetic wraps as in Go; `now.Sub`
  saturates as in Go. Go's `int64(float64)` is amd64's `CVTTSD2SQ` (NaN and
  out-of-range values give the minimum `int64`), not Rust's saturating `as`,
  which would accept a fill interval Go skips (`1e9 / 9.25e18` tokens a
  second panics in Go; a saturating conversion returns a bucket).

- **D-103 Real time in Go's synctest tests.** Go runs most of
  `parallel_test.go` in `testing/synctest` bubbles (fake time). Here they run
  on real time with Go's bounds: `TestDetachable`'s second case takes about
  10 s against its 11 s bound. `TestParallelCancel` sleeps 100 ms per job
  instead of 1 s (it counts jobs, not time). juju's
  `TestNoBonusTokenAfterBucketIsFull` advances a fake clock 5 s instead of
  sleeping.

- **D-104 `mysql_dsn` lives in `gorse-common`.** `gorse-storage-core` depends
  on `gorse-common`, and `log` needs `redact_db_url`, which is in `mysql_dsn`.
  Putting `mysql_dsn` in storage-core would make `gorse-common` depend on it,
  a cycle. The module moved with `git mv`, and storage-core re-exports it as
  `pub use gorse_common::mysql_dsn;`, so `gorse_storage_core::mysql_dsn` keeps
  its path.

- **D-105 Run-time log routing.** The three Go loggers are replaced when
  flags are read. The Rust set-up uses one `tracing` subscriber with a reload
  handle; the access and OpenAI logs are chosen by target inside the writer
  (`Route::make_writer_for`). A per-layer `with_filter` panicked when layers
  were swapped in at run time ("Filtered layer used, but it had no FilterId"),
  so routing moved into the writer.

- **D-106 Datasets under `~/.gorse`.** `datautil` keeps Go's layout:
  `dataset/<name>` is the unpacked directory and `temp/` holds the download.
  `LoadIris` reads `iris.data` from it. Its target class is `i64`, Go's `int`
  on the 64-bit targets Gorse runs on.

- **D-107 Test-only dependencies.** `tempfile` 3.27.0 (dev), and the
  `[workspace.dependencies]` additions are `tracing-subscriber` 0.3.23 (`fmt`,
  `json`, `registry`, `std`), `csv` 1.4.0, `reqwest` 0.13.5 (`blocking`,
  `rustls`), `zip` 9.1.3 (`deflate`). Their rows are in
  `porting/crate-selection.md`; the statik row's `zip` is bumped from 9.0.4 to
  9.1.3 so both uses agree.

- **D-108 The test servers use `std::net`, not go-restful or axum.** Go's mocks answer through
  go-restful over `net/http`. The Rust mocks answer through a small HTTP/1.1 server on `std::net`
  (`http.rs`). It serves one request per connection, reads `Content-Length` bodies, and answers with
  `Connection: close`. The mocks serve two routes each. `gorse-common` has no async runtime, so axum
  and tokio would add two dependencies to serve two routes.

- **D-109 Client calls end when their context does.** `http::post` runs the request on its own
  thread and checks the context every 10 ms. A context that is done returns `Error::Context`, as Go's
  cancelled request does. The request thread finishes unobserved and its reply is dropped.

- **D-110 The clients use reqwest's blocking client with rustls.** Go's `http.DefaultClient` is
  shared across requests, and so is the Rust `OnceLock` client. Both honour the proxy environment
  variables. No Phase 1 caller is async.

- **D-111 Param value type.** `ParamValue` has one variant per Go dynamic
  type Gorse stores: `Nil`, `Int` (Go `int`, `i64`), `Int32`, `Int64`,
  `Float32`, `Float64`, `Bool`, `String`, `IntSlice` (`Vec<i64>`). The accessors
  switch on the variant, which keeps Q-15 (JSON gives `float64`, so `get_int`
  falls back to its default). `Other(&'static str)` stands for any other Go
  type, named as Go prints it. The Go test uses `[]string{"hello"}` as its wrong
  type, so the Rust test uses `Other("[]string")`. `gob::Value`
  (`gorse_common::encoding::gob`) was not reused: it has no `Other`, and it
  belongs to another crate. The gob
  codec for model blobs and the meta store (WP-2-11) convert to and from
  `ParamValue` where they read and write `Params`.

- **D-112 Names.** Go's `ParamName` is a string type. Here names are `&str`
  arguments, `String` map keys, and the predefined names are `&str` constants
  with SCREAMING_SNAKE names (`N_EPOCHS` for `NEpochs`). Entries are in a
  `BTreeMap`, so iteration is by name.

- **D-113 Copy.** Go's `Params.Copy` is `Clone` here. `Overwrite` returns a
  new map and takes `&Params`.

- **D-114 Random generator.** `BaseModel` stores
  `Option<RandomGenerator>`, so `BaseModel` has `Default`. `SetParams` seeds it
  from `RandomState` (0 when missing or not an `int64`), as Go does.

- **D-115 Downloads.** The download uses a `reqwest` blocking client with
  its timeout turned off. Go's `http.Get` has no timeout, and reqwest's blocking
  client gives up after 30 seconds (`reqwest` 0.13.5,
  `src/blocking/client.rs`, `Timeout::default`). Integrator ruling (2026-10-28): accepted. `datautil.rs` must match, which is O-25.

- **D-116 Home directory.** `gorse_dir`, `dataset_dir` and `temp_dir` are
  functions that read `$HOME` on each call (X-55). Go computes them once in
  `init`. Tests use `locate_built_in_dataset_in` and `create_dirs` with
  temporary directories, because setting `$HOME` in a test is `unsafe` in
  edition 2024 and the workspace denies `unsafe`.

## Open questions for Ritchie

- **Datasets.** The network policy of this environment blocks
  `https://cdn.gorse.io`, where the Go model tests and the SDK test setup download
  their datasets (`PORTING_NOTES.md` O-1). Phase 3 metric parity needs either
  that host allowed or the dataset files placed in the repository's environment.
