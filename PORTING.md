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
| 1. Foundation | in progress | Done: WP-1-1, WP-1-2, WP-1-3, WP-1-4, WP-1-5, WP-1-6, WP-1-7, WP-1-8, WP-1-9, WP-1-10, WP-1-11, WP-1-12, WP-1-13, WP-1-14, WP-1-16, WP-1-17, WP-1-18, WP-1-19. WP-1-15: TOML, dotenv, config.go, YAML and the log swap merged (D-78, D-82 to D-95, X-48 closed); the frozen config.schema.json and config.keys.md match a fresh run of the Go generator byte for byte (X-49, closed 2026-10-28). WP-1-18 (dataset) is merged: Q-210 to Q-217, X-117 to X-129, O-51 and O-52 open, and the ml-1m test is ignored (O-1). Every Phase 1 WP is merged. All in `porting/work-packages.md` |
| 2. Storage | in progress | Done: WP-2-1 (data core; its shared suite waits for the SQL and MongoDB stores, O-36), WP-2-4 (cache core; O-28), WP-2-7 (MongoDB cache store, feature `mongodb`; creator registered, D-157), WP-2-8 (vector core, async, exact search; O-30) and WP-2-10 (blob, async; O-31 closed by 3966bf6f with the in-process XML listener tests). WP-2-5 (Redis cache store, feature `redis`, Redis 8, cluster and Kvrocks; X-103 honoured per D-158). WP-2-3 (MongoDB data store, feature `mongodb`, MongoDB 6.0.28; O-53 open for 8.0; the `get_item_stream` doc is corrected, O-54). WP-2-11 (meta store, SQLite through rusqlite 0.39.0, D-160 to D-166; O-48 closed, O-49 and O-50 open for Phases 3 and 4). WP-2-2 (SQL data store, MySQL, Postgres and SQLite as one build; D-177 to D-181; O-36 closed by the SQL and MongoDB runs; O-57 and O-59 closed by the Go and Rust cross-reads on PostgreSQL, MySQL and SQLite, D-177 to D-181). WP-2-6 (SQL cache store: MySQL and Postgres on sqlx 0.9.0, SQLite on rusqlite 0.39.0; D-167 to D-175; Q-232 MySQL auth open). WP-2-9, 2-12, 2-13 not merged |
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

- **C-30 The shared suite is run by the xvec tests only.** `testQuantization`
  in `database_test.go` is called only from the Qdrant and Milvus suites. It is
  ported in `suite.rs` and run for xvec with `QuantizationNone`, the one
  quantization xvec accepts (`xvec::tests::quantization_none`). WP-2-9 calls
  it with the other quantization types.

- **C-31 `Database` is an async trait with the caller's context.** Every
  Go method takes `context.Context`; the Rust trait takes
  `&gorse_common::gocontext::Context` first and is `#[async_trait]`, as the
  integrator ruled (D-122). This replaces the synchronous trait of the first
  head. The xvec store does its file IO and scans on tokio's blocking pool
  (`spawn_blocking`), and holds no std lock across an `.await`. WP-2-9 and
  WP-2-12 use the same shape.

- **C-32 The Redis crate moves to 1.8.0, with `connection-manager`.** The crate choice in `porting/crate-selection.md` (`redis`, with `tokio-comp` and `cluster-async`) is unchanged. Its version was 1.7.1, and Ritchie's rule of 2026-10-17 requires the latest stable version (1.8.0; checked against the crates.io index). `connection-manager` is added so a store reconnects by itself, as go-redis does. `tokio-rustls-comp` enables `tls-rustls`, which gives `rediss://` its rustls TLS (X-102). `tls-rustls-insecure` gives `skip_verify=true` its insecure mode, which go-redis also honours (Q-202). `base64` (0.23.1) is an optional dependency of the `redis` feature, for category encoding. `tokio` is a normal dependency of the crate, since the blob stores need it, and the Redis store's lazy connection uses its `sync` feature.

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

- **D-117 `Labels` is Go's `any` as a Rust enum.** Labels reach Gorse as two
  Go types. The REST decoder (go-restful v3.13.0, `entity_accessors.go:138`)
  calls `UseNumber`, so its numbers are `json.Number`. gorm's JSON serializer
  (gorm v1.31.1, `schema/serializer.go:95`) calls `json.Unmarshal`, so values read
  from the database are `float64`. `ValidateLabels` treats the two differently
  (it accepts `json.Number`, rejects `float64` and `bool`), so the enum keeps
  `Number(String)` (text) and `Float64(f64)` apart. `Deserialize` gives
  `Float64`, which is `json.Unmarshal`'s default. The REST path needs its own
  `UseNumber` decode (Q-178).

- **D-118 `Categories` is `Option<Vec<String>>`.** Go's nil slice marshals
  as `null`, and an empty slice as `[]`. Both occur: a REST body without
  `Categories` gives nil, and `sql.go` writes nil as `"null"`. A `Vec` would
  change the wire output. `ItemPatch.Categories` uses the same type, because
  `patch.Categories != nil` is how Go decides to update.

- **D-119 `Labels::Nil` means unset in patches.** `ItemPatch.Labels` and
  `UserPatch.Labels` are not `Option`. Go's `patch.Labels != nil` treats a nil
  `any` as unset, and `Labels::Nil` is that value.

- **D-120 Go's `sort.Sort` is ported, not Rust's sort.** `sort.Sort` is
  pattern-defeating quicksort and not stable. `SortFeedbacks` decides which
  feedback is latest when timestamps are equal, and `logics/recommend.go`,
  `worker/pipeline.go`, `server/rest.go` and `master/rest.go` all sort with it.
  Up to 12 elements it is an insertion sort, which is stable, but above that the
  tie order depends on the exact sequence of Less and Swap calls. So
  `gorse_common::gosort` is a port of go1.27.0's `sort.go` and
  `zsortinterface.go` (`Sort`, pdqsort and its helpers). It is a shared module
  for later packages, which also call `sort.Sort`.

- **D-121 `ScanOptions`, `ScanOption`, `GetOptions` are not in this WP.**
  They are query options of the `Database` interface, not record types, and
  WP-2-1 ports them with the interface. `NewScanOptions` and the `With*`
  functions go there too.

- **D-122 Storage traits are `async`.** Every storage family (cache, vectors,
  blob, data, meta) exposes an `async` trait. Traits used as `dyn` (`Open`
  returns one) use `async-trait` (`porting/crate-selection.md`). Reason: the
  network stores (Redis, MySQL, Postgres, MongoDB, Qdrant, Milvus, S3-style blob)
  need async clients, and `tokio` is already the runtime that `tonic`, `axum`
  and `sqlx` require. One rule for every family keeps the callers the same.
  Rules that follow:
  - No sync facade over a network driver. A blocking wrapper would hide the cost
    and nest runtimes.
  - Each method keeps Go's `ctx` as its first parameter, `&gocontext::Context`.
    It checks `ctx.err()` on entry and between units of work. Dropping the
    future cancels the rest. Each store records this in its X-entries.
  - In-process stores (sqlite through `rusqlite`, the xvec file log) run their
    blocking work in `tokio::task::spawn_blocking`. A `std` lock is never held
    across an `.await`.
  - `tokio` joins the library's dependencies when the first backend needs it.
    WP-2-4 keeps it a dev-dependency until then.
  Accepted from the cache core (WP-2-4), which already follows this. The vector
  core (WP-2-8) keeps a sync trait in its notes (C-31), which this decision
  overrides: it must be converted before it merges. Decided by the integrator on
  the coordinator's request (2026-10-28). It binds WP-2-1 to WP-2-13.

- **D-123 The `Database` trait is `async` through `async-trait` (0.1.92,
  MIT/Apache-2.0, already in Cargo.lock).** Go's interface is used as a value
  (`Open` returns it), so the trait must be dyn-compatible; native `async fn`
  in traits is not. Storage WPs 2-1 and 2-8 face the same choice; this is the
  pattern to follow. Adding `async-trait` to `porting/crate-selection.md` is the
  integrator's call.

- **D-124 tokio is 1.54.1, not the 1.53.3 in the crate-selection table.**
  Ritchie's rule is the latest stable release, and 1.54.1 was published on
  2026-10-21. Tokio is a dev-dependency of `gorse-storage` for `#[tokio::test]`
  until a backend needs it in the library.

- **D-125 Context is the `gocontext` handle, passed by reference.** Go's
  `ctx context.Context` is the first parameter of each method. Deadlines do not
  exist in `gocontext`, so the suite's `WithTimeout` test uses a context that a
  timer thread cancels (see X-72).

- **D-126 `Registry` replaces Go's package-level map.** Go registers
  creators in `init`. Rust has no `init` for this, so the node builds a
  `Registry` and registers the backends it was compiled with.

- **D-127 `Error` carries Go's `errors.Is` as `Error::kind()`.** Sentinels
  are `gorse_storage_core::errors::Error`. `DocumentCondition` is Go's
  `fmt.Errorf("document condition: %w", ...)`. `Backend` is an
  `Arc<dyn Error>`, so `Error` stays `Clone` for `ReturnValue`, which hands its
  error out on every read, as Go's `ReturnValue` does.

- **D-128 `ScorePatch` fields are `Option`.** Go's nil `Categories` (no
  change) is `None`, and an empty non-nil slice (clear) is `Some(vec![])`.

- **D-129 Trait object.** `Database: Send + Sync` with `&self` methods, so
  `open` returns `Box<dyn Database>`. Go's mutable `sync.Map` state sits behind
  one `RwLock` per store and one per collection. The store is an
  `Arc<Store>`: each async method clones the `Arc` and its owned inputs, and
  runs the sync `Store` method on the blocking pool (`blocking` in `xvec.rs`,
  which resumes a task's panic on the caller). Nothing borrowed crosses an
  `.await`.

- **D-130 Errors.** `vectors::Error` is `Storage { kind, message }` (the
  storage sentinel plus Go's exact text, `kind()` is Go's `errors.Is`),
  `Context(ContextError)`, and `Other(String)` for everything else. The
  messages copy Go's `fmt.Errorf` texts, including the `collection X: not
  found` and `collection X already exists` forms.

- **D-131 Registry.** Go fills `creators` from `init` functions. Rust lists
  the backends in `vectors::open`, which is the same lookup by prefix. WP-2-9
  adds its prefixes there.

- **D-132 Enums.** `Distance` is an enum. `QuantizationType` is an enum with
  an `Other(String)` case, because Go's type is a string and `master/tasks.go`
  converts unchecked config strings into it.

- **D-133 Integers.** Go's `int` fields and arguments (dimensions, bits,
  `topK`, counts) are `i64`, as in `gorse-storage-core`, so the Go range checks
  stay.

- **D-134 Time.** `Vector.Timestamp` is `gotime::Time`. Storage keeps Go's
  `UnixMilli`.

- **D-135 Own file format.** D-6 decided that `xvec://` is Rust's own
  format. Each collection is a directory `<prefix><name>` holding one
  `gorse.vectors` log: a header (magic `GVECTORS`, version 1, dimension, distance
  code) and one record per upsert or delete. Writes are appended and synced
  before they are applied. A torn last record is dropped on open. `Optimize`
  rewrites the log. The layout is in the module header of `xvec.rs`.

- **D-136 Exact search.** The store scans every live vector and keeps the
  best `topK`. HNSW, which Go uses, is not ported (X-75).

- **D-137 async stores, ctx first (D-122).** `Store` and `BlobWriter` are
  `#[async_trait]`. Every method takes `ctx: &gocontext::Context` first and checks
  `ctx.err()` on entry and between units of work (each write, each list entry).
  No store holds a runtime and nothing calls `block_on`. No sync facade.

- **D-138 `done` is a oneshot.** `create` returns a
  `tokio::sync::oneshot::Receiver<()>`. Go closes `done`; the port sees a
  `RecvError` because the sender is dropped when the writer is closed or dropped.

- **D-139 POSIX IO through `tokio::fs`.** Each `tokio::fs` call runs on the
  blocking pool, which is the `spawn_blocking` the ruling asks for. The gRPC
  server's temp-file creation and rename call `spawn_blocking` directly.

- **D-140 cloud writes.** A writer buffers up to 5 MiB (the S3 minimum part
  size). Data up to that size goes up in one request on close. More starts a
  multipart upload with at most 4 parts in flight.

- **D-141 cancelled master uploads are aborted.** The master client runs each
  upload on its own task. A cancelled context aborts that task, so the stream is
  reset instead of ended. The server persists only a stream that ends normally,
  so a cancelled upload creates no blob (test `cancelled_upload_persists_nothing`).

- **D-142 NotFound.** `BlobError::kind()` maps POSIX `ErrorKind::NotFound`
  and object_store `NotFound` to `gorse_storage_core::errors::Error::NotFound`.
  gRPC missing-blob errors are not mapped (O-34).

- **D-143 crates.** `async-trait` 0.1.92 (dyn-compatible async methods, the
  crate-selection row), `tokio` 1.54.1 (the latest; this WP moved the crate from
  1.53.3 to 1.54.1 to match that row) with `fs`, `io-util` and `time`,
  `object_store` 0.14.2 (`aws`, `gcp`, `azure`), `tokio-stream` 0.1.19,
  `futures-util` 0.3.34, `bytes` 1.12.1, `url` 2.5.8, `percent-encoding` 2.3.2.
  Dev: `reqwest` (workspace) to seed the GCS emulator. `tokio-util` is not used.

- **D-144 gRPC list uses a background context.** The gRPC `ListBlobs` call
  carries no Go context. A dropped call is the only cancellation.

- **D-145 Callback lifetimes for `async-trait` (WP-2-4, fixed in WP-2-7).** The
  `scan` and `scan_scores` callbacks of the cache `Database` trait take explicit
  higher-ranked lifetimes: `for<'a> FnMut(&'a str)` and
  `for<'a, 'b, 'c> FnMut(&'a str, &'b str, &'c str, Time)`. The elided form
  (`FnMut(&str)`) is rewritten by `async-trait` to one named lifetime, so a backend
  that passes a borrowed string fails with E0597. Every backend hits this. The
  change is the signature only. Approved by the coordinator as a government decision
  (2026-10-28). Merged as `0c9bf1bb` (fix commit `9e3aa927`). The Redis and SQL
  threads take it by cherry-pick.

- **D-146 The data `Database` trait is `#[async_trait]`, ctx first (D-122).** Methods that Go gives a `ctx` take `&Context` first. Lifecycle methods Go gives no `ctx` (`Close`, `Ping`, `Init`, `Reconcile`, `Optimize`, `Purge`) take none.

- **D-147 Go's chan pairs are one `BoxStream<'a, Result<Vec<T>, Error>>`.** `GetUserStream`, `GetItemStream` and `GetFeedbackStream` return a stream of batches. An error is the last item, which matches the order Go's consumers see on the two channels. The stream is borrowed from `&self` and `&Context`, and dropping it stops the scan.

- **D-148 Go's `int` counts are `i64`** (`n`, `batch_size`, the delete count, the counts), as the cache core uses `i64` for Go `int`. Go's `int` can be negative, and the backends must see the same value.

- **D-149 The registry is the cache core's.** `Creator` is a sync `fn` that returns `Box<dyn Database>`, and `Registry::open` takes the longest matching prefix. Go's `Open` picks among matching prefixes in map order, which is random, so the longest is the only deterministic choice that matches every path a prefix can match unambiguously.

- **D-150 `ScanOption` is a newtype over `Box<dyn Fn(&mut ScanOptions) + Send + Sync>`**, like storage-core's `Opt`. Each `with_*` function is a Go `With*` function.

- **D-151 Async registries (ruling, applied in a later commit).** Every registry
  whose `Creator` opens a store with I/O takes an async creator. The cache and data
  registries (D-146 onward; the cache core's D-126) use
  `pub type Creator = for<'a> fn(&'a str, &'a str, &'a [Opt]) -> BoxFuture<'a, Result<Box<dyn Database>, Error>>`,
  with `BoxFuture` from `futures-util`, and `Registry::open` is `async`. Every
  backend's creator wraps its async open. The vector and blob registries follow
  the same rule where they have a creator. Reason: MongoDB, SQL and Redis open
  their clients asynchronously, so a sync creator would need a blocking facade over
  a network client, which D-122 forbids. `cache::open` stays the one entry point,
  as Go's `Open` is. Until the applying commit lands, a store may build against
  this signature. Integrator, 2026-10-28; the coordinator asked for it (O-42).

- **D-152 The driver is `mongodb` 3.9.1.** It is the latest stable release on
  2026-10-28 (`cargo info mongodb`) and Apache-2.0, as `porting/crate-selection.md`
  already says. It is an optional dependency of `gorse-storage`, behind the
  `mongodb` feature, so a build with no MongoDB store does not link it. The
  driver's default BSON is `bson` 2.15.

- **D-153 `MongoDB::open` is `async`, and the registry does not hold it.**
  Go's creator calls `mongo.Connect`, which is lazy. The Rust driver's
  `ClientOptions::parse` is `async`, and `Client::with_options` spawns tasks on
  the runtime. So `MongoDB::open(path, table_prefix)` is an async function. It
  is not registered in `cache::Registry`, whose `Creator` is a sync `fn`. See
  O-42.

- **D-154 The callbacks of `scan` and `scan_scores` take any lifetime.** The
  trait bounds are `for<'a> FnMut(&'a str)` (and `for<'a, 'b, 'c>` for
  `scan_scores`), which the approved fix in `9e3aa92` sets. `async-trait`
  rewrites the elided lifetime inside `FnMut(&str)` into a named lifetime, so a
  backend cannot call the callback with a string it borrowed from a row
  (rustc E0597). Redis (WP-2-5) and SQL (WP-2-6) take the same commit.

- **D-155 Each write is its own round trip.** The driver's `bulk_write` needs
  MongoDB 8.0, and the suite runs on 6.0.28. Each write is an
  `update_one(...).upsert(true)`, in order, and the first error stops the batch.
  That is what Go's ordered bulk write does on the server.

- **D-156 Collections and indexes match Go.** Collections are
  `<prefix>values`, `<prefix>documents` and `<prefix>time_series_points`. `init`
  creates the values collection, then the two indexes on documents and the one
  on points, with the keys Go uses. On mongo:6 (6.0.28), `getIndexes()` on the
  Go-created and the Rust-created `gorse_cache_test` lists the same index names:
  `collection_1_subset_1_id_1`, `collection_1_subset_1_categories_1_is_hidden_1_score_-1`
  and `name_1_timestamp_1`.

- **D-157 D-151 applied (integration).** `cache::Creator` and `data::Creator` are
  `for<'a> fn(&'a str, &'a str, &'a [Opt]) -> BoxFuture<'a, Result<Box<dyn Database>, Error>>`,
  and `Registry::open` is `async` in both. `vectors::open` is `async` too; its xvec
  directory reads run through `xvec::blocking` (spawn_blocking). The registry test is
  `#[tokio::test]`. `cache::register_builtin` registers the backends this build was
  compiled with (`mongodb://` and `mongodb+srv://` with the `mongodb` feature), as
  Go's `init` does; a node calls it once on the registry it builds. Backends in
  WP-2-2, 2-3, 2-5, 2-6 and 2-9 write their creators to this signature.

- **D-158 X-103 ruling: `skip_verify=true` is honoured (coordinator).** The
  coordinator ruled on 2026-10-28 that a `rediss://` URL with `skip_verify=true`
  turns certificate verification off, as Go does. The operator opts in through
  their own URL, and a drop-in must accept the configs Go accepts. The default
  stays verified. The Redis branch tests both settings and logs the behaviour as
  a reproduced quirk. This overrides the integrator's recommendation to refuse
  the option, because the plan forbids behaviour changes.

- **D-159 O-48 ruling: arrays and objects round-trip (coordinator).** Go
  stores model params as `interface{}`, so an array or object param survives a
  load and a save. The port must do the same. The ruling adds a `ParamValue`
  variant that holds a JSON value for arrays and objects, in the model-core file.
  The meta thread (WP-2-11) makes that change. A golden test checks that a
  Go-written JSON value re-encodes byte-identically.

- **D-160 rusqlite is 0.39.0, not the latest 0.41.0 (integrator).** The rule
  is the latest stable crate. `sqlx` 0.9.0, also the latest, reaches
  `libsqlite3-sys` through `sqlx-sqlite`, which needs a version below 0.38, and
  cargo refuses a graph that holds two `links = "sqlite3"` packages. Checked with
  `cargo generate-lockfile` on a scratch crate that has `sqlx` 0.9.0 (mysql and
  postgres, sqlite off) and each rusqlite: 0.41.0 and 0.40.2 fail with the
  `links` error, and 0.39.0 resolves with `libsqlite3-sys` 0.37.0. So 0.39.0 is
  the newest rusqlite this graph holds (the lower 0.40.x releases were not tested).
  The meta merge and the SQL branch (WP-2-6) use this pin. Revisit when
  `sqlx-sqlite` accepts `libsqlite3-sys` 0.38 or later.
  The coordinator upheld this on 2026-10-28 and asked whether to drop rusqlite
  and use sqlx's SQLite driver instead. The integrator keeps rusqlite, for three
  reasons. First, sqlx-sqlite is in the same graph and pins the same
  `libsqlite3-sys` (below 0.38), so the SQLite engine version is 0.37's either
  way: dropping rusqlite changes no version. Second, the meta store (D-162,
  merged and gated) and the SQL cache's SQLite path (WP-2-6) are written on
  rusqlite with `spawn_blocking`, and their tests pass there. Third, moving them
  to sqlx-sqlite would rewrite two gated stores for no version gain.

- **D-161 The meta traits are async, `Open` is async.** D-122. `Database` is
  `#[async_trait]` (dyn-compatible, since `Open` returns one), with no `ctx`
  because Go's `meta.Database` takes none. Every method runs on
  `tokio::task::spawn_blocking` (`porting/crate-selection.md`, rusqlite row).

- **D-162 SQLite is `rusqlite` (bundled), not `sqlx`.** The crate-selection
  row for gorm sqlite. The store has one connection behind a std `Mutex` that
  only blocking tasks lock.

- **D-163 The time text is Go's.** modernc writes `t.String()` of the UTC
  time (`conn.go` `formatTime`, default `writeTimeFormat` is empty) and reads the
  `String()` layout, with an optional `m=` suffix, then its seven layouts
  (`conn.go` `parseTime`). The port does the same, so Go and Rust rows in one
  file compare alike under `datetime('now', ...)`. Golden: `update_time_text_matches_go`,
  `update_time_is_stored_as_go_stores_it`, `go_written_rows_are_listed_and_pruned`.

- **D-164 `Node` and `Model` are plain structs.** `Node.Type` is `r#type`.
  `ListNodes` returns `Vec<Node>` (Go's `[]*Node`). `Model` JSON keeps Go's field
  names (`ID`, `Type`, `Params`, `Score`) through serde renames.

- **D-165 `FromJSON` merges `Params` into the map it holds.** This follows
  Go's `json.Unmarshal` into a map: keys the JSON names are replaced, the others
  stay, and `null` clears the map. Absent fields keep their values, and a `null`
  for `ID`, `Type` or `Score` is a no-op. Golden: `from_json_matches_go`.

- **D-166 A decoded array or object is `ParamValue::Json`.** Resolves
  O-48 (coordinator decision, 2026-10-28 12:00Z). `FromJSON` keeps the
  `gojson::Value` of an array or object, so it encodes again as Go encodes the
  value: numbers as float64s, object keys sorted by bytes, the same escapes as
  `json.Marshal`. The accessors treat it as a type mismatch, as Go's do for a
  `[]any`. Golden: `round_trip_matches_go` (8 cases, Go's bytes), and
  `from_json_matches_go` compares the array values too.

- **D-167 Drivers.** MySQL and Postgres run on `sqlx` 0.9.0 with explicit SQL
  per dialect. SQLite runs on `rusqlite` on `tokio`'s blocking pool, behind
  `Arc<Mutex<Option<Connection>>>`; the mutex is held only inside the blocking
  closure, never across an `.await`. The trait is the async D-122 trait.

- **D-168 rusqlite is 0.39.0, not the latest 0.41.0.** `sqlx` 0.9.0 (the
  latest) resolves `sqlx-sqlite`, which needs `libsqlite3-sys` below 0.38. The
  `links = "sqlite3"` rule allows one `libsqlite3-sys` per graph, so `rusqlite`
  0.41 cannot resolve with it. This departs from the latest-versions rule for
  `rusqlite` only. Ruled by D-160 (integrator). After the merge of f74ce29b the
  `sqlite` feature is empty, since the meta store links `rusqlite` unconditionally.

- **D-169 SQL is written out per dialect.** The statements copy the ones GORM
  issues: `ON DUPLICATE KEY UPDATE ... VALUES()` on MySQL, `ON CONFLICT ... DO
  UPDATE SET ... excluded.x` on Postgres and SQLite, `JSON_CONTAINS` on MySQL and
  SQLite (a deterministic `json_contains` registered on each SQLite connection,
  Q-25), and `@>` on Postgres `text[]`.

- **D-170 Init is not GORM AutoMigrate.** Init runs `CREATE TABLE IF NOT
  EXISTS` for the three tables, and on MySQL `ALTER TABLE ... ADD INDEX`, ignoring
  duplicate-index error 1061 as Go does. MariaDB cannot index `CAST(... AS CHAR
  ARRAY)`, so that index is skipped there as in Go. Postgres adds `btree_gin` and a
  GIN index on `(collection, subset, categories)`. Column alterations that GORM
  would make on an existing table are not reproduced.

- **D-171 Categories.** A nil list writes NULL; an empty list writes `"[]"` on
  MySQL and SQLite and `'{}'` on Postgres. An empty patch writes `"[]"`.

- **D-172 Purge.** Purge deletes values and documents only. Time-series points
  survive, as in Go.

- **D-173 Pool.** `ApplySQLPool` sets the maximum open connections and the
  connection lifetime. Go's `MaxIdleConns` has no sqlx counterpart and is read and
  dropped. When no maximum is set, sqlx uses its default of 10; Go's default is
  unlimited.

- **D-174 Connection glue.** `sqlconn` holds the MySQL DSN parsing, the
  per-connection session variables, the pool options and `json_contains`. The SQL
  data store (WP-2-2) shares it; reconcile at merge if that store lands its own.

- **D-175 Async registry creators.** The integrator applied D-151 on integration
  (1a14f9c, D-157). The SQL creators use the async `Creator` and register in
  `cache::register_builtin`, which the merge commit wires.

- **D-176 Court acceptance on the merged head (integrator, 2026-10-28).** The
  coordinator's rule: for a backend WP the integrator runs the backend's
  acceptance command on the merged head, and does not rely on the citizen's run
  alone. The integrator started MongoDB 6.0.28, MySQL 8.0.46, PostgreSQL 10.0 and
  Redis 8.10.3 with Docker (`mongo:6`, `mysql:8.0`, `postgres:10.0`, `redis:8`).
  On the head `a88ec014` (the merge of WP-2-6 `bd14c40` and its fold; the code is
  the same as `1e5449f7`):
  - WP-2-3, `MONGO_URI=mongodb://root:password@127.0.0.1:27017/ cargo test -p
    gorse-storage --features mongodb data::mongodb`: 8 passed, 0 failed, 0 skipped,
    including `mongo` (the shared suite).
  - WP-2-7, `MONGO_URI=... cargo test -p gorse-storage --features mongodb
    cache::mongodb`: 2 passed, 0 failed, 0 skipped, including `mongo`.
  - WP-2-6, `MYSQL_URI=mysql://root:password@tcp(127.0.0.1:3306)/
    POSTGRES_URI=postgres://gorse:gorse_pass@127.0.0.1:5432/ cargo test -p
    gorse-storage --features mysql,postgres,sqlite cache::sql`: 9 passed, 0 failed,
    0 skipped; `mysql_suite`, `postgres_suite` and `sqlite_suite` all run.
  - WP-2-5, `REDIS_URI=redis://127.0.0.1:6379/ cargo test -p gorse-storage
    --features redis cache::redis`: 12 passed, 0 failed, 0 skipped, including
    `redis_suite`. Run on the same head as a retrospective check; the Redis merge
    e1a14b8d was gated without it.
  - Gate on `a88ec014` with the four URIs set: `cargo fmt --all --check`,
    `cargo clippy --workspace --all-targets -- -D warnings`, `RUSTDOCFLAGS='-D
    warnings' cargo doc --workspace --no-deps`, the config and yaml goldens, `cargo
    test --workspace --no-fail-fast` (905 passed, 0 failed, 13 ignored, 0 skipped)
    and `go vet` in `contracts/tools`: all exit 0.
  Environment, not code: the MySQL root account (`root@%`) was switched to
  `mysql_native_password`. On `caching_sha2_password` a cold server cache over plain
  TCP fails with `RSA auth backend disabled` (X-144, Q-232), and the store code is
  unchanged. The MongoDB container also stopped once when the host disk filled; it
  was restarted and the runs repeated. The runs above are the ones that count.

- **D-177 one build, all three dialects.** The store is compiled only when `mysql`, `postgres` and `sqlite` are all on (`cfg(all(...))` in `data/mod.rs` and in `data::register_builtin`). Go links all three drivers into every binary. The alternative was a `cfg` on every `Pools` variant and every dispatch arm.

- **D-178 explicit SQL on sqlx 0.9.** One generic query layer (`sql/query.rs`) binds every value (`Bind`). The dialect differences are written out per module. Text passed to `sqlx::query` is wrapped in `AssertSqlSafe` (sqlx 0.9 requires the assertion for any dynamic text). Each wrap is justified at the site: the text holds only literals, the table names from the operator's prefix, and placeholders. The `SET` statements come from the operator's DSN.

- **D-179 `json_contains` through the SQLite C API.** sqlx-sqlite 0.9 has no scalar function API, so `sql/sqlite.rs` calls `sqlite3_create_function_v2` on each connection's handle in `after_connect`. The `unsafe` is confined to that module (`#![allow(unsafe_code)]` with a reason; a `SAFETY:` comment on every block). Argument and error rules come from `gorse_storage_core::json_contains`.

- **D-180 streams run on a task.** Each stream runs its query on a spawned task and sends batches over `mpsc(1)`, as Go's `GetItemStream` sends over a goroutine with `bufSize = 1`. The context is checked before each batch. The item stream keeps items at or after `time_limit` (`>=`), as Go does.

- **D-181 `AssertSqlSafe` and the audit.** See D-178. No request data is ever written into SQL text. Every user value is bound.

- **D-182 A pre-release RSA crate for MySQL cold logins (integrator).** The
  `mysql` feature enables `sqlx/mysql-rsa` (X-144, which matches Go's cold
  `caching_sha2_password` login over plain TCP). That feature pulls `rsa`
  0.10.0-rc.19, a pre-release, with its `pkcs1`, `pkcs8` and `crypto-bigint` 0.7
  line, and the lock moves `ctutils` 0.4.3 to 0.4.6. The latest-stable rule cannot
  hold on this path: `rsa` 0.10.0-rc.19 is the newest release of the 0.10 line on
  the index, and sqlx 0.9.0 requires it. The client only encrypts the password with
  the server's public key and never decrypts with a private key, so the timing
  issue in the 0.9 line (RUSTSEC-2023-0071) is not on this path. Accepted as a
  logged deviation (X-153). Revisit when `rsa` 0.10 has a stable release, and run
  `cargo audit` when it is installed.

- **D-183 Court acceptance for WP-2-2 and X-144 (integrator, 2026-10-28).** On the
  merged head `5b80e256`, with the services of D-176 (MySQL 8.0.46, PostgreSQL 10.0,
  MongoDB 6.0.28, Redis 8.10.3):
  - WP-2-2, `MYSQL_URI=mysql://root:password@tcp(127.0.0.1:3306)/
    POSTGRES_URI=postgres://gorse:gorse_pass@127.0.0.1:5432/ cargo test -p
    gorse-storage --features mysql,postgres,sqlite data::sql`: 3 passed, 0 failed,
    0 skipped (`test_mysql`, `test_postgres`, `test_sqlite`).
  - SQL cache, `cargo test -p gorse-storage --features mysql,postgres,sqlite
    cache::sql` with the same URIs: 10 passed, 0 failed, 0 skipped. This includes
    `mysql_cold_full_authentication_connects` (X-144), which creates a fresh account
    so its first login is the cold full authentication.
  - `cargo clippy -p gorse-storage --all-targets --features mysql,postgres,sqlite
    -- -D warnings`: exit 0.
  - Gate on `5b80e256` with all four URIs set: 905 passed, 0 failed, 13 ignored;
    fmt, clippy, rustdoc, goldens and go vet exit 0.
  The SQL data thread's cross-reads (Go populates, Rust reads, and back, on
  PostgreSQL 10.0, MySQL 8.0 and SQLite, 9 of 9 identical) are recorded in
  `PORTING_NOTES.md` under WP-2-2 and close O-57 and O-59. Pre-release `rsa` is
  D-182 and X-153.

- **D-184 Court acceptance on the merged head `12f19e46` (integrator, 2026-10-28).**
  The head holds the audit lock bump (597b8198, from 1b01c01) and the notes fold.
  Services: MySQL 8.0.46, PostgreSQL 10.0, MongoDB 6.0.28, Redis 8.10.3, rustfs
  (`rustfs/rustfs:alpha`), Azurite 3.34.0, fake-gcs-server 1.56.1, MariaDB 10.2,
  Kvrocks nightly, and a six-node Redis 8 cluster on ports 7000 to 7005.
  - Gate (`cargo fmt --check`, `clippy --workspace --all-targets -D warnings`,
    rustdoc with `-D warnings`, the config and YAML goldens, `cargo test --workspace`
    with the four URIs, `go vet`, cloud and MongoDB clippy): every step exit 0.
    TEST section: 905 passed, 0 failed, 13 ignored.
  - Blob, `S3_ENDPOINT=127.0.0.1:9000` (host and port, no scheme, as the CI sets it)
    with `--features s3,azure blob`: `test_s3` and `test_s3_multipart` pass, and
    `test_azure_blob_emulator` passes. The `gorse-test` bucket and container were
    created before the run. Azurite 3.34.0 rejects the Azure SDK's newest API
    version, so the setup client pins `2023-11-03`.
  - Blob, GCS, `--features gcs blob -- --include-ignored`: the XML put and multipart
    tests pass in process. `test_gcs_read_list_remove` and `test_gcs` fail on the
    emulator, because fake-gcs-server 1.50.2, 1.52.3 and 1.56.1 all answer 404 for
    the XML list request (`GET /gorse-test?list-type=2`), so the read path is not
    run here (O-62). `test_gcs` is `#[ignore]` (O-31).
  - Blob, GCS, re-run for O-62 on this head: `--features gcs blob` with
    `GCS_EMULATOR_ENDPOINT=http://127.0.0.1:5051`, the fake-gcs-server started with
    `-external-url`. `test_gcs_read_list_remove` passes against it. The in-process
    `test_gcs_xml_round_trip` and `test_gcs_xml_missing_object_is_not_found` pass, as do the
    put and multipart tests. `test_gcs` stays `#[ignore]` (O-31, O-62). The cloud suite
    `--features s3,gcs,azure blob` gives 16 passed and 1 ignored, with the S3, Azure and GCS
    emulators all live. The Go-against-Rust comparison is in PORTING_NOTES.md under O-62.
  - Redis, `cache::redis` (12 of 12 each): Kvrocks at `redis://127.0.0.1:6666/`;
    the six-node cluster as `redis+cluster://127.0.0.1:7000` and with the five
    `addr=` parameters. No skips.
  - MariaDB 10.2, `MYSQL_URI=mysql://root:password@tcp(127.0.0.1:3307)/`:
    `data::sql::tests::test_mysql` passes. `cache::sql` gives 9 passed and 1 failed:
    `mysql_cold_full_authentication_connects` runs `CREATE USER ... IDENTIFIED WITH
    caching_sha2_password`, and MariaDB 10.2 has no such plugin (syntax error 1064).
    The case is a MySQL 8 check (X-144), and Go's `TestMySQL` has no equivalent. It
    is not closed by this entry; see O-61, and the coordinator rules on it.
  - Verdict: the head is accepted for Redis (Kvrocks and the cluster) and for the
    data and cache stores on MySQL 8.0 and PostgreSQL 10.0. The blob S3 and Azure
    tests pass. The GCS read path and the MariaDB cache case are open. D-185 rules on
    both.

- **D-185 Coordinator rulings on O-61 and O-62 (coordinator, 2026-10-28; applied by the
  integrator).**
  - O-61, approved with one addition. The cold-login test asks the server whether
    `caching_sha2_password` exists (`information_schema.PLUGINS`). If it does, as on
    MySQL 8, the case is unchanged. If it does not, as on MariaDB 10.2, the same
    cold-login case runs with an account on the server's default plugin, and the path
    is printed as `cold login path: ...` (shown with `--nocapture`). The test then
    reads the account's `mysql.user.plugin` and checks that it is the printed plugin.
    MariaDB stores its default plugin as an empty string, so an empty value counts as
    `mysql_native_password`. The test is not skipped on either server, and it never
    passes silently. The change is in `crates/gorse-storage/src/cache/sql.rs`.
    Evidence: `cache::sql` on MariaDB 10.2.44 gives 10 of 10, with the native path
    printed. On MySQL 8.0.46 and PostgreSQL 10.0 it gives 10 of 10, with the
    `caching_sha2_password` path printed. Clippy with `-D warnings` and `cargo fmt
    --check` exit 0. O-61 is closed.
  - O-62, open. The in-process XML listener must also cover the GCS list and read
    requests, checked against what Go's client sends, so the read path has runnable
    evidence. The emulator test stays open under O-62 for a run with a working
    emulator or real GCS. The blob thread is merged and not reused, so the work needs a
    new brief. It does not block Phase 3 prep, but it must close before the Phase 2
    exit is declared.

- **D-186 The Go tool dumps the Rust databases; the Rust mirror is a second check (WP-2-13,
  D-WP213-1).** `schemadump`'s full run creates its own databases, so it cannot dump one a
  Rust store made. The coordinator approved a `-dump-only` mode in `contracts/tools/schemadump`
  (`-dump-only <kind>` with `-target`, `-database` and `-postgres-container`). It writes one dump
  to stdout and changes nothing. The kinds are `sqlite`, `mysql`, `postgres`, `mongodb` and
  `redis`. The full run's outputs are unchanged: before and after the change, all four backends
  give byte-identical schema files, apart from the wall-clock `FT.INFO` `total_indexing_time`,
  and the sample files differ only in `_id` and `update_at`. The Rust mirror in `schema::dump`
  stays as a second check, so a fault shared by the Rust stores and a mirror of the Go format
  cannot hide a diff. Each Rust-initialised database must equal the Go-initialised dump and
  `contracts/schema/` under both checks.

- **D-187 PostgreSQL is dumped inside the server container (D-WP213-2).** `pg_dump -s` runs
  through `docker exec` in `POSTGRES_CONTAINER`, as `dump-schemas.sh` does. The host's
  `pg_dump` is version 16 and would not match the server's 10.0 output. The default container
  name is `storage-postgres-1`, the compose project's name. On the integrator's host the service
  container is `gorse-pg`, so the acceptance ran with `POSTGRES_CONTAINER=gorse-pg`. No code
  change.

- **D-188 Separate databases (D-WP213-3).** Go writes `gorse_schema_*` and Rust writes
  `gorse_rs_schema_*` (`schema::init`), so neither build reads the other's database. Each test
  drops and creates its own databases before it starts.

- **D-189 A missing backend fails, it does not skip (D-WP213-4).** Each test needs its URI
  (`MYSQL_URI`, `MARIADB_URI`, `POSTGRES_URI`, `MONGO_URI`, `REDIS_URI`) and fails without it,
  as `conformance/README.md` requires. The Go tests skip instead. SQLite needs no server.

- **D-190 Run-dependent values and read counts (D-WP213-5).** Two rules, applied to the Go side,
  the Rust side and the contract. `FT.INFO`'s `total_indexing_time` is replaced by
  `<run-dependent>`, and it is the only normalisation. Every `FT.INFO` call increments
  `number_of_uses`, so each dump reads a freshly built index once: the Go tool reads first, the
  index is rebuilt, and the Rust mirror reads second. Redis 8 then agrees on `FT.INFO documents`.

- **D-191 MariaDB 10.2 has no frozen dump (D-WP213-6).** `contracts/schema` holds MySQL 8.0
  only, so `schema_mariadb` compares the Go and Rust dumps with each other. The differences from
  the MySQL dump come from the server: the version line, `DEFAULT '0'` shown as `DEFAULT 0` for
  `double`, `labels` as `longtext` rather than `json`, and the multi-valued index
  `idx_collection_subset_categories`, which is missing on MariaDB.

- **D-192 Samples are not in the schema check (D-WP213-7).** `schemadump` also writes
  `samples/*.json`. WP-2-13 compares the schema files only, as its brief says. The rows are
  covered by the store WPs' cross-reads (D-177 to D-181 and the WP-2-3 and WP-2-5 runs).

## Open questions for Ritchie

- **Datasets.** The network policy of this environment blocks
  `https://cdn.gorse.io`, where the Go model tests and the SDK test setup download
  their datasets (`PORTING_NOTES.md` O-1). Phase 3 metric parity needs either
  that host allowed or the dataset files placed in the repository's environment.
