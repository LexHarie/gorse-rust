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
| 2. Storage | exited (D-207) | Done: WP-2-1 (data core; its shared suite waits for the SQL and MongoDB stores, O-36), WP-2-4 (cache core; O-28), WP-2-7 (MongoDB cache store, feature `mongodb`; creator registered, D-157), WP-2-8 (vector core, async, exact search; O-30) and WP-2-10 (blob, async; O-31 closed by 3966bf6f with the in-process XML listener tests). WP-2-5 (Redis cache store, feature `redis`, Redis 8, cluster and Kvrocks; X-103 honoured per D-158). WP-2-3 (MongoDB data store, feature `mongodb`, MongoDB 6.0.28; O-53 open for 8.0; the `get_item_stream` doc is corrected, O-54). WP-2-11 (meta store, SQLite through rusqlite 0.39.0, D-160 to D-166; O-48 closed, O-49 and O-50 open for Phases 3 and 4). WP-2-2 (SQL data store, MySQL, Postgres and SQLite as one build; D-177 to D-181; O-36 closed by the SQL and MongoDB runs; O-57 and O-59 closed by the Go and Rust cross-reads on PostgreSQL, MySQL and SQLite, D-177 to D-181). WP-2-6 (SQL cache store: MySQL and Postgres on sqlx 0.9.0, SQLite on rusqlite 0.39.0; D-167 to D-175; Q-232 MySQL auth open). WP-2-9 (D-197 to D-200), WP-2-12 (D-202 to D-206) and WP-2-13 (D-186 to D-193) merged. The court audit of the env-gated backend tests is O-66 (D-201). Phase 2 exited (D-207): `test_gcs` stays ignored with its reason, and O-62 stays open for a real-GCS run before Phase 5. |
| 3. Models and logics | in progress | Accepted: WP-3-1 (D-216), WP-3-4 (D-217), WP-3-2 (D-224), WP-3-3 (D-230, contract touches D-231), WP-3-5 (D-234, design D-233), WP-3-6 (D-229; Q-254 closed by D-232, Q-255 open) and WP-3-7 (D-236, design D-235). WP-3-8 needs the datasets (O-1, O-70 to O-72). The master must pass processed items (O-75). REST must confirm nil against empty results (O-76). O-30 is extended by WP-3-7 and stays open |
| 4. Nodes | in progress | WP-4-1 to WP-4-8; WP-4-1 accepted (D-264); WP-4-2 accepted (D-274); WP-4-3 accepted (D-275), O-75 closed, O-88 closed (D-276). WP-4-4 accepted (D-280), O-76 closed (D-279, D-280); X-251 is closed (D-281). O-87 is closed (D-282): X-245 and X-248 resolved, X-246 closed with no deviation, X-247 approved as a test-oracle tie grouping, X-249 resolved with the fixture precondition. X-252 is an upstream quirk, parity with Go, and a Phase 5 conformance watch item (coordinator ruling, 13:33). X-244 is closed by the vendored `mongodb` 3.9.1 with its document-sequence patch (C-34, D-272): the live Go-versus-Rust MongoDB deep-label test matches on the full gate. Open: X-239 (provisional), Q-265, and O-87 (two TEST failures under the parallel run, passing alone). WP-4-8 is `cmd/gorse-bench` (D-239). `cmd/goat` is not ported (D-238). WP-4-7 stays blocked on the SDK dataset under O-1 (D-237) |
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

- **C-33 The brief counts two frappe tests in `data_test.go` (WP-3-2, correction C-WP32-1).** The Go file has one, `TestLoadDataFromBuiltIn`, which is ported as `load_data_from_built_in_frappe`. The brief said two.

- **C-34 The `mongodb` driver is vendored with a 69-line patch (WP-4-2, X-244, D-272).** The published `mongodb` 3.9.1
  sends an update's statements inline, in the command body, so the server names an element of a refused update from
  `updates.0.` and counts its depth two levels deeper than Go's driver does (D-273). Go sends the statements as an OP_MSG
  document sequence. `third_party/mongodb-3.9.1` is 3.9.1 unchanged, with its licence, except for two files: `src/coll/options.rs`
  adds `UpdateModifications::Raw`, and `src/operation/update.rs` sends `updates` as the sequence. It is wired in with
  `[patch.crates-io]` in the root `Cargo.toml`, and `third_party/mongodb-3.9.1/GORSE.md` says why. It is a fork of a
  third-party crate, with the same reason as C-16 (`rustls`). Drop it when the upstream driver can send update statements as
  a document sequence.

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
    `GCS_EMULATOR_ENDPOINT=http://127.0.0.1:5051`, the fake-gcs-server 1.56.1 started with
    `-public-host` and `-external-url` both set to `127.0.0.1:5051`. `test_gcs_read_list_remove`
    passes against it. The in-process
    `test_gcs_xml_round_trip` and `test_gcs_xml_missing_object_is_not_found` pass, as do the
    put and multipart tests. `test_gcs` stays `#[ignore]` (O-31, O-62). The list filters on
    the raw prefix as Go does (Q-240), tested by `test_gcs_xml_list_matches_go_raw_prefix`.
    The cloud suite `--features s3,gcs,azure blob` gives 17 passed and 1 ignored, with the S3,
    Azure and GCS emulators all live. The Go-against-Rust comparison is in PORTING_NOTES.md
    under O-62.
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

- **D-193 Court acceptance for WP-2-13 on the merged head (integrator, 2026-10-28).** The WP
  merged as `c2f5ff88` (head `eb343ec`, which supersedes `dcc8bee`), and the notes were folded
  in `a88eb4eb`. The head at the gate is `23b31693`.
  - Schema, `cargo test -p gorse-conformance schema` with `MYSQL_URI` (MySQL 8.0.46),
    `MARIADB_URI` (MariaDB 10.2.44, `mysql://root:password@tcp(127.0.0.1:3307)/`),
    `POSTGRES_URI` (10.0), `MONGO_URI` (MongoDB 6.0.28) and `REDIS_URI` (Redis 8):
    18 passed, 0 failed. That is 12 unit tests and six integration tests
    (`schema_sqlite`, `schema_mysql`, `schema_mariadb`, `schema_postgres`,
    `schema_mongodb`, `schema_redis`). `POSTGRES_CONTAINER=gorse-pg` was set, because the
    default `storage-postgres-1` does not exist on this host (D-187).
  - `gofmt -l` on `contracts/tools/schemadump` lists nothing, and `go vet` exits 0.
  - Gate on `23b31693` with all five URIs, `MARIADB_URI` and `POSTGRES_CONTAINER` set: TEST
    section 958 passed, 0 failed, 13 ignored. fmt, clippy, rustdoc, goldens, `go vet`, cloud
    clippy and MongoDB clippy all exit 0. An earlier gate run without `MARIADB_URI` was stopped
    before it finished. Its schema tests would have failed for want of that URI, so it is not
    a result.
  - The count moved from 905 to 958. Of the 53 extra, 18 are the conformance tests. The other 35
    are storage tests that the conformance crate's feature set (`mysql`, `postgres`, `sqlite`,
    `mongodb`, `redis`) compiles into the workspace test build, because the resolver unifies
    features across members. I did not trace those 35 to individual tests.
  - Verdict: accepted. The open items are O-63 (samples and the Redis key layout) and O-64
    (mixed-version runs).

- **D-194 Court acceptance for O-62 on the merged head (integrator, 2026-10-28).** The O-62
  head `f145a51` is merged as `23b31693`, which also carries `435b9fc` (lists by the raw
  prefix, as Go does).
  - Cloud suite, `cargo test -p gorse-storage --features s3,gcs,azure blob`, with RustFS
    (`S3_ENDPOINT=127.0.0.1:9000`), Azurite 3.34.0 and fake-gcs-server 1.56.1 started as
    `-scheme http -port 5051 -public-host 127.0.0.1:5051 -external-url http://127.0.0.1:5051`
    (the memory backend), with bucket `gorse-test` created through the JSON API: 17 passed, 1
    ignored. The ignored test is `test_gcs`, which stays under O-62 and O-31 because the server
    refuses object_store's XML upload.
  - `test_gcs_read_list_remove` passes. The XML list `GET /gorse%2Dtest?list-type=2&prefix=blob%2F`
    returns 200 when `-public-host` is set to the address the client uses. The blob thread
    found that this flag is needed, and I reproduced it.
  - Correction to D-184 and D-185. The 404s for the XML list recorded in D-184 for fake-gcs-server
    1.50.2, 1.52.3 and 1.56.1 came from runs without `-public-host`. They do not show that those
    versions cannot serve the XML list. The `test_gcs` refusal of the XML upload is not re-checked
    here.
  - Verdict: accepted for the in-process listener and for the list, read and remove path against
    fake-gcs-server 1.56.1 with `-public-host`. O-62 stays open for `test_gcs` only.

- **D-195 Metric-parity tolerance for Phase 3 (proposed by the integrator, 2026-10-28; for the coordinator's ruling).**
  The plan's Phase 3 exit gate asks for NDCG, precision and recall of every model within "the
  tolerance the Go tests themselves assert (`benchDelta`)", on the same dataset files.
  The facts. The Go CF tests assert NDCG within `benchDelta` = 0.01 of 0.36 on ml-1m
  (`model/cf/model_test.go`). The CTR tests assert accuracy within `classificationDelta` = 0.01
  (Frappe at least 0.919 − 0.01, ml-tag 0.815), and Criteo 0.77 within 0.025, and within 0.02 for
  the clone (`model/ctr/model_test.go`). No Go test asserts precision or recall. Every dataset
  these tests load comes from `cdn.gorse.io`, which fails to connect from this container.
  `files.grouplens.org` also fails, and no copy of any dataset exists on disk (O-1).
  The proposal has two gates.
  1. Metric gate, on the datasets. For each model and dataset a Go test uses, the Rust metric
     must lie within the Go test's own delta of the Go test's own reference value. The deltas
     are not widened. For precision and recall, which Go does not assert, the reference is a
     Go run of the same configuration on the same dataset file, and the band is `benchDelta`
     (0.01). Reasoning: the Go tests fit with `SetJobs(NumCPU)`, so Go's own values vary from
     run to run. The deltas are the band the project already accepts for that variation and for
     implementation differences. Widening a delta until a port passes would be a weakened test,
     which the no-cheating rule forbids.
  2. Replay gate, runnable offline. A Go generator fits small synthetic datasets, which it
     writes to a file, with one job and a fixed seed. It records the fitted parameters and the
     per-epoch metrics. Rust must reproduce them within 1e-5 absolute, which allows for f32
     summation order. The thread records the largest difference. Reasoning: the Rust port uses
     the Go `math/rand` port (`gorse_common::util::rand`), so with one job the random streams
     should match. The thread must confirm that. A 0.01 band on NDCG would hide algorithm drift
     that the replay catches. A difference above 1e-5 is a finding for the court, not a reason
     to widen the bound.
  Until the datasets are provided, only gate 2 can run here. The gate 1 runs need the six dataset
  archives (`ml-1m`, `ml-100k`, `pinterest-20`, `ml-tag`, `frappe`, `criteo`) placed under
  `/mnt/project-files/datasets/` with their SHA-256 sums recorded. Ritchie or the coordinator
  must supply them.

- **D-196 Phase 3 scope and crate versions (integrator, 2026-10-28).**
  - Scope. `model/ctr/fm.go` is ported. `model/ctr/fm_xla.go` (build tags `cgo && xla`) and
    `model/ctr/model.py` are not, as D-8 already says. Each WP's notes record the omission.
  - Crate versions, from the crates index on 2026-10-28. `rquickjs` 0.14.0 is the planned version.
    `minijinja` is 3.0.4, not the 3.0.1 that `porting/crate-selection.md` listed. Newer patch
    releases came out after the plan was written, and the rule is the latest stable, so the row
    is corrected here and in `porting/crate-selection.md`. `bitset` (bits-and-blooms, used by
    `model/cf/model.go`) becomes `fixedbitset` 0.5.7, unless the thread shows a behaviour
    difference. `golang-set` becomes `std::collections::HashSet`, with no crate.
  - goptuna's TPE sampler is a hand port (`crate-selection.md`), as WP-3-3 says.
  - Dependency correction. `work-packages.md` lists WP-3-5 after WP-3-4 only, but
    `logics/item_to_item.go` compiles expressions with the expr-lang VM (line 130). WP-3-5
    therefore also waits for WP-3-6. The `SuggestParams` methods that take a goptuna `Trial`
    (`model/cf/model.go` lines 397 and 588, `model/ctr/fm.go` line 79) are ported in WP-3-3.

- **D-197 Qdrant client is `qdrant-client` 1.20.0 (WP-2-9, P-1).** `porting/crate-selection.md`
  named 1.19.0. 1.20.0 is the latest stable release (checked in the crates index on 2026-10-28),
  so the row is 1.20.0. Go's module pins `qdrant/go-client` v1.18.2. Against the 1.20.0 server
  the 1.18.2 client only warns, so the Rust client follows the server's release.

- **D-198 Milvus and Weaviate are hand-written REST adapters (WP-2-9, P-2).** This is D-17 as
  already written. No change.

- **D-199 Storage errors wrap the storage sentinels, as Go's `%w` does (WP-2-9, P-3).** Every
  NotSupported error from the three stores is `Error::Storage { kind: NotSupported, .. }` through
  `vectors::not_supported`, so `kind()` matches Go's `errors.Is`. Qdrant and Weaviate returned
  `Error::Other` for these before the fix.

- **D-200 Court acceptance for WP-2-9 on the merged head `4b835df1` (integrator, 2026-10-28).**
  The branch `claude/wp-2-9-vector-backends` is merged from `3918b84`, as one commit, because the
  three stores share `open()`, the features and `Cargo.lock`.
  - Acceptance, run by the integrator with all three servers set: `QDRANT_URI=qdrant://127.0.0.1:6334
    MILVUS_URI=milvus://127.0.0.1:19530 WEAVIATE_URI=weaviate://127.0.0.1:8080 cargo test -p
    gorse-storage --features qdrant,milvus,weaviate vectors`: 56 passed, 0 failed, 1 ignored (the
    Weaviate sparse case, which Go's `TestSparse` also skips). The thread's own count is the same.
  - The servers: Qdrant `latest` (1.20.0), Weaviate `semitechnologies/weaviate:1.35.7` from Docker
    Hub (`cr.weaviate.io` does not resolve from this container), etcd `v3.5.25` from
    `gcr.io/etcd-development/etcd` (`quay.io` does not resolve), and Milvus v2.6.9 with the
    compose settings: rustfs as the object store and `MQ_TYPE=woodpecker`. The thread ran Milvus
    with local storage and rocksmq instead. The compose-style run passes, so the thread's
    deviation from compose is not needed for the result.
  - The Go oracle and the two-way cross-read are the thread's evidence (TestMilvus 11 of 11,
    TestQdrant 10 of 10, TestWeaviate with TestSparse skipped). The integrator did not rerun
    them. The cross-read harness is not committed, so it cannot be rerun from the repository.
    That is a gap to close with the next Phase 2 change, not a reason to reject.
  - Verdict: accepted, with the harness gap recorded.

- **D-201 Court audit of the env-gated backend tests, and the gate on `479760b2` (integrator,
  2026-10-28).** The coordinator's court rule (17:18): a backend test with no server configured
  reports ignored or fails, and never passes. The audit is O-66. It found that the Redis, MongoDB
  and SQL tests returned early when their URI was unset, so they passed with no server. They now
  panic with `<NAME> is not set`, and the Redis TLS cases are `#[ignore]` with their reason
  (`4ea540a5`). The WP-2-9 thread's head `b9fde43d` makes the 30 vector backend tests `#[ignore]`
  with a reason, and their helpers fail when the URI is unset. It is merged as `479760b2`. The
  integrator's own edits to the vector files were dropped in favour of this version, which follows
  the same rule.
  - Gate on `479760b2`, with every URI set (MARIADB, MYSQL, POSTGRES, MONGO, REDIS, QDRANT,
    MILVUS, WEAVIATE, and `GCS_EMULATOR_ENDPOINT=http://127.0.0.1:5051` for fake-gcs-server).
    `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, rustdoc with
    `-D warnings`, the config and YAML goldens, `go vet`, cloud clippy and Mongo clippy all exit 0.
    `cargo test --workspace --no-fail-fast`: TEST section 992 passed, 0 failed, 48 ignored. The
    gate sums only that section. The earlier gate failed `fmt --check` on the module order in
    `vectors/mod.rs`, which `4ea540a5` fixes.
  - Backend acceptance, run by the integrator with `--include-ignored`: `cargo test -p
    gorse-storage --features qdrant,milvus,weaviate vectors -- --include-ignored --skip
    vectors::weaviate::tests::sparse`: 69 passed, 0 failed. Of these, 30 are the backend tests
    (Milvus 11, Qdrant 10, Weaviate 9), and 39 are the xvec, database, proxy and no-database
    tests. The thread's 56 is the same run before the WP-2-12 proxy vectors were merged, since
    56 = 30 + 22 + 3 + 1.
  - The negative check: `env -u REDIS_TLS_URI -u REDIS_TLS_UNTRUSTED cargo test -p gorse-storage
    --features redis tls -- --include-ignored` fails both cases with `REDIS_TLS_URI is not set`
    (exit 101). They fail rather than pass.
  - The unset-URI check, run before the commit: with the URIs unset, the Mongo cases fail (4), the
    MySQL cases fail (3), the Postgres cases fail (2), the Milvus cases fail (11), the Weaviate
    cases fail (9, the sparse case ignored), the Qdrant cases fail (10), and the Redis cases fail
    (6). `test_gcs_read_list_remove` panics without `GCS_EMULATOR_ENDPOINT`. `test_gcs` stays
    ignored under O-31 and O-62.
  - The cross-read, `cargo test -p gorse-conformance --test vector_cross_read -- --ignored`: both
    directions pass with the three servers (2 passed).
  - Verdict: accepted. The audit is complete for the backends listed in O-66.

- **D-202 The proxy suites run through the proxy (WP-2-12, P-1).** The coordinator approved an
  additive `run_through_proxy` in `data/database_test.rs` and in `cache/database_test.rs`. It runs
  the cases of `run()` in the same order, minus the ones Go's `ProxyTestSuite` skips. `run()` is
  unchanged. The suites through the proxy run only with `--features mysql,postgres,sqlite`, as the
  SQL suites they wrap do. `refuses_init_scan_and_purge` (data and cache) asserts the client-side
  refusals with Go's texts: `method Init is not allowed in ProxyClient` and `method Purge is not
  allowed in ProxyClient` (data), and `init`, `scan` and `purge is not allowed in proxy client`
  (cache).

- **D-203 A NotFound error keeps Go's text (WP-2-12, P-2).** `data::Error` gains
  `Wrapped { kind, message }`. Its `Display` is the message and `kind()` reports `NotFound`. The
  proxy client returns it for a missing item or user, so the text is `item <id> not found` (and
  `user <id> not found`), as Go's `fmt.Errorf("item %s %w", ...)` gives it. Go's text is not
  shortened, since error text can reach REST bodies and logs. Test:
  `missing_item_reads_back_with_go_text`.

- **D-204 `describe_collection` on a missing collection (WP-2-12, P-3).** The server answers with
  gRPC `NotFound`. The client maps that code to a NotFound-kind error whose text is
  `collection <name>: not found`. The transcript pins `collection missing: not found`.

- **D-205 gRPC error text follows Go's client (WP-2-12, P-4).** Go's client shows a status as
  `rpc error: code = <Code> desc = <msg>`, with Go's spelling of the code names (`Canceled`, not
  `Cancelled`). The Rust client builds the same text. Server errors that are not NotFound go out
  as `Status::unknown(err.to_string())`, as Go's server does.

- **D-206 Court acceptance for WP-2-12 on the merged head `479760b2` (integrator, 2026-10-28).**
  The branch `claude/wp-2-12-proxies` is merged as `50886475`, one `--no-ff` merge of `892767b`.
  That head replaces the earlier `d122a96`, and it runs the Go-Rust wire check under `cargo test`
  (O-68).
  - Acceptance on the final head: `cargo test -p gorse-conformance proxy` passes 3 of 3 (the
    golden matches Go's regenerated transcript, Go's client runs against the Rust servers, and the
    Rust clients run against Go's servers). `cargo test -p gorse-storage proxy` passes. In the
    workspace run, with the SQL and Mongo URIs set, 36 proxy-named tests pass, and the SQL suites
    run through the proxy pass too (D-201).
  - The check needs `go` on PATH. The gate ran with `/usr/local/go/bin` on PATH.
  - The notes are folded as D-202 to D-205, Q-241 to Q-245, X-161 to X-165 and O-67 to O-69.
    `porting/notes/WP-2-12.md` is deleted.
  - Verdict: accepted. O-69 stays open, since the scenario does not cover `SearchItems`,
    `GetLatestItems` or the three vector backends.

- **D-207 Phase 2 exits, and `test_gcs` stays ignored with its reason (coordinator ruling,
  integrator, 2026-10-28).** The coordinator ruled at 17:36 that `test_gcs` is blocked by the
  emulator itself: fake-gcs-server refuses the XML upload that `object_store` makes. The GCS write,
  list, read and remove paths have in-process and emulator evidence. So:
  - `test_gcs` stays `#[ignore]`, with the reason `fake-gcs-server refuses the XML upload that
    object_store uses (O-31, O-62)`.
  - O-62 stays open for a real-GCS run before Phase 5.
  - Phase 2 is exited on the merged head `e1b51b37`. All thirteen storage packages, WP-2-1 to
    WP-2-13, are merged. Their evidence is D-184 (blob S3 and Azure, Redis Kvrocks and cluster,
    MariaDB), D-185 (O-61), D-193 (WP-2-13), D-194 (O-62), D-200 (WP-2-9), D-201 (the audit gate on
    `479760b2`) and D-206 (WP-2-12).
  - Carried into later phases, still open: O-53 (MongoDB 8.0 untested), O-60 (the two SQLite
    drivers, after conformance), O-63 (samples and the Redis key layout), O-64 (mixed-version runs),
    O-65 (Weaviate sparse vectors) and O-69 (proxy transcript coverage).

- **D-208 CF models are ported in `gorse-model` (WP-3-1, base and scope).** The branch
  `claude/project-thread-1slm94` starts from `46c5703`, which contains `020f2ede`, so the brief's
  base holds. `BPR.SuggestParams` and `ALS.SuggestParams` need goptuna's `Trial`, and WP-3-3 ports
  them, so they are left out here (D-209).

- **D-209 `SuggestParams` of BPR and ALS waits for WP-3-3 (WP-3-1, scope).** Their line ranges in
  `model/cf/model.go` (397 to 405 and 588 to 596) are listed in the thread's notes.

- **D-210 The brief's contract reference for `cf.MarshalModel` is corrected (WP-3-1, integrator).**
  The brief named `contracts/model-blobs.md` as the contract for `cf.MarshalModel`. That file says
  its CF section describes `logics.MatrixFactorizationUsers`, and that `cf.MarshalModel` "is only
  used by tests at the pinned commit; it is not a contract". `MarshalModel` is ported in Go's format
  anyway, because the Go tests round-trip through it. The brief in
  `/mnt/project-files/briefs/phase-3-briefs.md` now says so.

- **D-211 BPR epoch jobs share one lock (WP-3-1, decision).** Go's BPR epoch jobs update the factors
  without locks, which races with `Jobs > 1`. Here each job takes one lock on the factors and runs
  its update whole. With `Jobs = 1` this is Go's exact sequence. With `Jobs > 1` the result depends
  on scheduling in Go too, so it is not expected to match bit for bit.

- **D-212 ALS rows are solved from a snapshot (WP-3-1, decision).** Go's ALS jobs in one sweep write
  disjoint rows and read only the other side, so there is no race. Each row is solved in a local
  copy and written back when the sweep ends. This gives Go's result for any `Jobs`.

- **D-213 BPR `cost` is not computed (WP-3-1, decision).** Go accumulates
  `math32.Log1p(math32.Exp(-diff))` into `cost[workerId]` and never reads it. Dropping it changes no
  output, so `Log1p` is not needed by this WP.

- **D-214 Dictionaries are copied into the model (WP-3-1, decision).** Go's model shares the split's
  `FreqDict` pointer. The Rust model copies it, counts included, so a later change to the split does
  not change the model. Go's `assert.Equal(trainSet.GetUserDict(), m.GetUserIndex())` holds for the
  copy.

- **D-215 The gob parameter conversion is private to `cf` (WP-3-1, decision).**
  `gorse_common::encoding::gob::Params` and `gorse_model_core::params::Params` are different types,
  with no conversion between them. `to_gob` and `from_gob` stay private to `cf/model.rs`. WP-3-2
  needs the same conversion, so it moves into `gorse-model-core` when WP-3-2 merges.

- **D-216 Court acceptance for WP-3-1 on the merged head `6d8928ed` (integrator, 2026-10-28).** The
  branch `claude/project-thread-1slm94` is merged as `f713e7f8`, at head `bce13a1`, which includes
  the first head `1e26fdc`. The court's first verdict (17:37) rejected `1e26fdc`: the replay golden
  was regenerated by hand, which breaks the Go-interop rule. `bce13a1` adds
  `conformance/tests/cf_golden.rs`, which runs `go run ./contracts/tools/modelreplay -mode cf -epochs
  5` into a temporary directory and requires its three files to equal the committed copies. It fails
  with the reason when Go is missing.
  - The golden check, with Go on PATH: `cf_oracle_matches_golden` passes. Its generator output
    matches the committed files byte for byte.
  - The replay, `cargo test -p gorse-model cf::`: BPR and ALS each compare 815 values, with the
    largest difference 0 against a tolerance of 1e-5 (D-195).
  - The mutation check (BPR learning rate 0.05 to 0.051) fails the replay at 8.93e-4, from the
    thread's run.
  - The gate on `6d8928ed`, with every URI set, is the same for D-216 and D-217: `cargo fmt
    --check`, `cargo clippy --workspace --all-targets -D warnings`, rustdoc with `-D warnings`, the
    config and YAML goldens, `go vet` and the two clippy runs with features all exit 0. `cargo test
    --workspace --no-fail-fast`: TEST section 1009 passed, 0 failed, 50 ignored. The vector
    acceptance (69 passed), the vector cross-read (2 passed) and the proxy checks pass. The Redis
    TLS cases fail with `REDIS_TLS_URI is not set` under `--include-ignored` (exit 101), as O-66
    requires. An earlier run on this head had 20 failures, all backend tests that could not reach
    their servers, because the Docker daemon was down after a worker restart. The daemon and the
    containers were restarted, and this run is the record.
  - Verdict: accepted. `O-70` and `O-71` stay open, and `X-166` to `X-172` are recorded.

- **D-217 Court acceptance for WP-3-4 on the merged head `6d8928ed` (integrator, 2026-10-28).** The
  branch `claude/project-thread-upnvrj` is merged as `6d8928ed`, at head `b18fee9`. Its first head
  `a0e5b86` was never pushed. Two verdicts were issued, and this head meets both.
  - The items blob's timestamp is a gob `time.Time`, a `GobEncoder` value, so its bytes match Go's
    (the coordinator's ruling, 17:37). `gob_golden.json` has 72 encodings, and
    `gob_decode_golden.json` has 6 more verdicts. `X-174` is resolved.
  - The sparse oracle runs live: `conformance/tests/sparse_oracle.rs` overlays
    `conformance/testdata/sparse_oracle_test.go` into package `logics` with `go test -overlay`, so
    the Go tree is unchanged. It checks 420 cases bit for bit. Without Go it fails. The committed JSON
    snapshot and its generator are removed.
  - The mutation check flips the pruner's tie rule, and the live oracle fails at case 4. The rule
    is restored, and the file is byte-identical to the commit.
  - `contracts/model-blobs.md` is updated for the `time.Time` value, since `contracts/` is
    integration-owned. The `MatrixFactorizationUsers` and `MatrixFactorizationItems` types are the
    `logics` package's own, as the thread notes.
  - The gate is the one recorded in D-216.
  - Verdict: accepted, with `Q-246` to `Q-250` and `X-173` to `X-178` recorded.

- **D-218 O-69 closes with the proxy follow-up, and the oracle purges its Qdrant server
  (WP-2-12 follow-up, integrator).** The follow-up `68418e35` on `claude/wp-2-12-proxies` adds
  `SearchItems`, `GetLatestItems` (all, by category, and after a time) and the vector proxy against
  a live Qdrant 1.20.0 server. Both interop directions run against it, and the golden transcript is
  regenerated at 88 lines. It is merged as `621a6f5e`, and O-69 is closed.
  - The first gate on the merge failed the three proxy wire checks in the workspace run.
    `list_collections` returned `["docs", "test"]` where the golden expects `["docs"]`. The vector
    suite leaves its last collection on the shared test server, because Go's `SetupTest` purges
    only before each case, and the suite does the same. The oracle's fixture purge deleted only
    `docs`.
  - The integrator's fix, `718a0bc7`, has the oracle delete every collection on its own live server
    before the scenario. The Go-client-against-Rust-server check then failed the same way, because
    the Rust peer (`proxy_peer serve`) also deleted only `docs`. The peer now deletes every
    collection too, in `d9b779b9`. The golden is unchanged, and the three checks
    pass with a leftover collection present.
  - The gate that covers this merge is recorded in D-224.

- **D-219 The FM layers draw from a per-model `Rand` (WP-3-2, D-WP32-1).** The layers (linear,
  embedding lookup, attention) are built in `ctr::fm`, drawing in the order Go's `nn` layers draw.
  The `nn` global RNG is `pub(crate)` and not seedable, so the layers cannot come from it. The merged
  `nn` module is unchanged.

- **D-220 Per-epoch scores come from one process per epoch count (WP-3-2, D-WP32-2).** Go's `Fit`
  returns only its last score, so `modelreplay` fits each epoch count in a fresh child process.

- **D-221 The initial weights are random by default, as Go's are (WP-3-2, D-WP32-3).** Go seeds
  `math/rand` randomly unless `GODEBUG=randautoseed=0`, and then seeds it with 1. Rust keeps a random
  default seed, as Go does, and the replay sets `set_init_seed(1)`. See Q-251.

- **D-222 `AutoScaler` uses a port of Go's `math.Log1p` (WP-3-2, D-WP32-4).** Libm's result differs
  in the last bits, and the float32 results of the two languages must agree.

- **D-223 Context label ids are absolute, and user and item label ids are local (WP-3-2,
  D-WP32-5).** Go's `Dataset.Get` treats them so, and the port does the same in the unified index.

- **D-224 Court acceptance for WP-3-2 on the final head `6f67b01a` (integrator, 2026-10-28).** The
  branch `claude/project-thread-jto4gb` is merged as `00b4431f` at head `08a1d04`. The first head,
  `e365efb`, was not merged, because its generator conflicted with WP-3-1's. The thread then merged
  `b688fd34` and kept both modes in one generator.
  - The Go replay golden is checked in conformance. `ctr_oracle_matches_golden` runs
    `GODEBUG=randautoseed=0 go run ./contracts/tools/modelreplay -mode ctr` and requires the output
    to equal `crates/gorse-model/testdata/ctr/afm_replay.json` byte for byte. It fails without Go. The
    replay's largest difference from Go is 0, against the tolerance of 1e-5 (D-195), and the thread's
    mutation of 1e-4 fails it.
  - The gate on `6f67b01a`, all URIs set: `cargo fmt --check`, `cargo clippy --workspace --all-targets
    -D warnings`, rustdoc with `-D warnings`, the config and YAML goldens, `go vet`, the cloud and
    Mongo clippy runs all exit 0. `cargo test --workspace --no-fail-fast`: TEST section 1036 passed, 0
    failed, 54 ignored. The vector acceptance (`--include-ignored`), the vector cross-read and the
    proxy checks (`proxy_wire` 3 of 3, and the storage proxy tests) pass. The Redis TLS cases fail with
    `REDIS_TLS_URI is not set` under `--include-ignored`, as O-66 requires.
  - The `schema` tests, the backend suites and the vector suites all ran with their servers up, after
    the Docker daemon was restarted, so the 20 backend failures seen on the thread's container do not
    arise here.
  - The gate covers the D-218 proxy fixes. The oracle and the Rust peer both delete every Qdrant
    collection before the scenario (`718a0bc7`, `d9b779b9`).
  - The contracts README names the CTR replay command. `O-72` stays open: the metrics test needs the
    datasets, as O-70 and O-71 do.
  - Verdict: accepted.

- **D-225 `expr-lang` 2.1.0 is rejected, and the expression engine is a hand port (WP-3-6, spike,
  D-WP36-1).** The brief's step 2 kept the crate only if it passed the Go oracle corpus (1330 cases,
  golden from expr-lang v1.17.8). Over the 1327 non-volatile cases: both reject 468 (391 at compile,
  77 at run time), the values agree on 652, the values differ on 166 (int against float from JSON, and
  Go type names), the spike rejects 27 cases where Go gives a value, accepts 12 that Go rejects at
  compile time, returns nil for 6 cases where Go gives an index error, and panics on 2 valid
  expressions. The reasons are: no static type checking, which Gorse needs (a score must be numeric
  and a filter boolean, `non_personalized.go`); error texts that differ from `file.Error`; no Go
  `time.Time` or `time.Duration` on values built from Go structs; panics on valid input; and no `;`
  sequences. So the checker, compiler and VM are a hand port (D-10), in `gorse-expr`.

- **D-226 The hand port follows Go's packages, file by file (WP-3-6, D-WP36-2).** `ty.rs` is the reflect
  type model, `nature.rs` is the checker's nature, `value.rs` is the runtime `Value` with one variant
  per Go kind, and `checker/`, `compiler.rs`, `vm/`, `builtin/` and `optimizer/` follow Go's files.
  Struct values are built from Gorse's data types in Rust, so that the `Ty` matches the Go struct.
  Not ported, as Gorse's API does not reach them: the patcher and `Visitor`, `ConstExpr`, the
  function and operator options, `Profile`, the `Debug` VM and `SafeFunction`. The typed fast-path
  opcodes are emitted only where Go's optimizer chooses them, and the port uses the generic opcode,
  whose results and errors are the same by construction (the corpus checks this, X-192).

- **D-227 Ruling on Q-254: `NonPersonalized` must be `Send` before the master merges (integrator,
  2026-10-28).** The coordinator asked for a ruling on how the master pushes into non-personalised
  recommenders. Go's `Push` is called from `parallel.Parallel` workers (`master/tasks.go`, 607 to
  630), so the port's `NonPersonalized`, whose compiled programs hold `Rc`, cannot be shared across
  those workers. The court requires the master's work package to make the expression types `Send`
  before it merges. The recommended way is `Arc`-based expression types, since Go pushes
  concurrently and a single owning thread would serialise the pushes. The change is local to
  `gorse-expr` and costs an atomic count per clone. The non-personalised port may keep `&mut self`
  until then. This is recorded as Q-254, open.

- **D-228 Ruling on Q-255: the `json.Number` labels stay floats for WP-3-6 and are fixed before
  Phase 5 (integrator, 2026-10-28).** Go keeps a `json.Number` as a named string type, so an expression
  that compares a numeric label with a string, or calls a string function on it, behaves differently
  here (X-185). The deviation is accepted for WP-3-6, because the rest of the expression semantics
  are exact on the oracle corpus. Before Phase 5 conformance, `gorse-expr` must carry a named-string
  value for `json.Number`, so the corpus can cover these cases. This is recorded as Q-255, open, and
  the coordinator schedules it.

- **D-229 Court acceptance for WP-3-6 on the head `faa048d4` (integrator, 2026-10-28).** The branch
  `claude/wp-3-6-logics-expr` is merged as `2ba2a1d6` at head `e2af7613`. The merged head failed
  `cargo fmt --check` on module formatting in `gorse-expr` alone, and `faa048d4` only applies
  `cargo fmt` to those sources. The fix changes no code.
  - The expression engine is the hand port of D-225 and D-226, with the rulings D-227 (Q-254, the
    `Send` requirement before the master merges) and D-228 (Q-255, the `json.Number` labels). Both
    stay open, and neither blocks this integration. The deviations are recorded as X-184 to X-192,
    and the Go oracle module as X-192. O-73 (the corpus compares map-order results as multisets)
    stays open. O-74 (`const_expr` is not run) is closed, because Gorse configures no `ConstFns`.
  - The gate on `faa048d4`, with every URI set: `cargo fmt --check`, `cargo clippy --workspace
    --all-targets -D warnings`, rustdoc with `-D warnings`, the config and YAML goldens, `go vet`, the
    cloud and Mongo clippy runs, the vector acceptance with `--include-ignored`, the vector cross-read,
    the proxy conformance and the proxy store tests all exit 0. `cargo test --workspace
    --no-fail-fast`: TEST section 1049 passed, 0 failed, 54 ignored. The count rises from 1036 on
    D-224's head, and the difference is WP-3-6's new tests.
  - The vector acceptance skips `vectors::weaviate::tests::sparse`, as D-224 and the Weaviate
    backend's own `#[ignore]` reason state. Go's `TestSparse` skips for Weaviate, so the skip matches
    Go and is not a removed check.
  - The Redis TLS cases fail with `REDIS_TLS_URI is not set` under `--include-ignored` with the URI
    unset (exit 101, expected, O-66). They fail rather than pass.
  - Verdict: accepted, with Q-254 and Q-255 open. The master work package must close Q-254 before the
    master merges, and Q-255 before Phase 5 conformance.

- **D-230 Court acceptance for WP-3-3 on the head `36aedd40` (integrator, 2026-10-28).** The branch
  `claude/wp-3-3-tuning` is merged as `1143819c` on the integration head `2b4e0942`. The merge was clean.
  The notes fold as Q-257, Q-258, X-193 to X-198, which closes X-65 (X-197). The port is goptuna v0.9.0's
  TPE sampler and Gorse's model search, with `SuggestParams` for BPR, ALS and AFM.
  - The Go oracle. `contracts/tools/tpegolden` writes `crates/gorse-model/testdata/tpe/golden.json`. The
    court regenerated it twice with `GODEBUG=randautoseed=0 go run ./contracts/tools/tpegolden`. Both
    runs equal the committed file byte for byte, and each other. The Rust tests read the committed file
    (`tpe::golden`). That is the pattern of the other committed goldens (util, tls and ratelimit), but
    unlike the CF and CTR oracles, which run Go inside the test, the regeneration is not run by the gate. Any change to the
    generator must regenerate the file by hand, as `contracts/README.md` says.
  - The contract touches. `gorse-common` makes `util::rand::math` public, with `exp` and `log` public.
    `gorse-model-core` gains `SuggestTrial` and `Model::suggest_params`. The first is a visibility
    change. The second is the X-65 change in a form the notes did not plan, since model-core cannot name
    goptuna's `Trial`. The coordinator approved both at 21:58, and the rulings are D-231.
  - No test is ignored, skipped or made to return early in the diff. The mock models in the test
    modules of `cf/optimize.rs`, `cf/evaluator.rs` and `ctr/optimize.rs` panic on methods the tests do
    not reach. The tests pass, so none of those panics is hit.
  - The gate on the merged tree, with every URI set: fmt, clippy, rustdoc, the config and YAML goldens,
    `go vet`, the cloud and Mongo clippy runs, the vector acceptance with `--include-ignored` (skipping
    `vectors::weaviate::tests::sparse`, as D-224 and D-229 record), the vector cross-read, the proxy
    conformance and the proxy store tests all exit 0. `cargo test --workspace --no-fail-fast`: TEST
    section 1083 passed, 0 failed, 54 ignored. That is 34 more than D-229's 1049, all from WP-3-3. The
    Redis TLS unset check exits 101, as O-66 expects.
  - Verdict: accepted. No open items. WP-3-3 is folded and its notes file is deleted.

- **D-231 Coordinator rulings on the two WP-3-3 contract touches (coordinator, 2026-10-28, 21:58).**
  The coordinator approved both touches on `36aedd40`, and the court logs them here as coordinator
  rulings.
  1. `gorse-common`: `util::rand::math` is public, with `exp` and `log` public. The change is to
     visibility and doc comments only, with no change in behaviour.
  2. `gorse-model-core`: the `SuggestTrial` trait, and `Model::suggest_params(&self, &mut dyn
     SuggestTrial) -> Params`. This deviates from X-65's plan, which put `SuggestParams` with goptuna's
     `Trial` on the trait. The reason is that model-core cannot name `goptuna.Trial`, which lives in
     `gorse-model`, a crate that depends on model-core. The deviation is X-197.

- **D-232 Court acceptance for Q-254 on `claude/q-254-send` at `df15b3f` (integrator, 2026-10-28).**
  The branch is based on the integration head `2b4e0942` and is merged as `6ae07e72` on `19a03820`
  (the WP-3-3 fold and D-231). The merge was clean. Q-254 is closed.
  - The change follows D-227. The expression types share their data through `Arc` (`Ty`, `Value`,
    `Nature.map`). The method cell of `Ty` is a `OnceLock`. The signatures do not change, so `push` and
    `pop_all` still take `&mut self`. The diff has no `unsafe` and no `Send` or `Sync` impl written by
    hand. Apart from the two checks below, each change is a rename or a reformat, and the corpus
    expectations are unchanged.
  - The compile-time checks are `programs_and_values_are_send` (`gorse-expr`: `Program`, `Value`, `Ty`)
    and `recommender_is_send` (`gorse-logics`: `NonPersonalized`). A type that is not `Send` stops the
    test crate from compiling, so each check is compile-time, and both run as tests that pass.
  - The gate on the merged tree, with every URI set: fmt, clippy, rustdoc, the config and YAML goldens,
    `go vet`, the cloud and Mongo clippy runs, the vector acceptance with `--include-ignored` (skipping
    `vectors::weaviate::tests::sparse`, as D-224 records), the vector cross-read, the proxy conformance
    and the proxy store tests all exit 0. The corpus golden test `golden_matches_go` passes. `cargo test
    --workspace --no-fail-fast`: TEST section 1085 passed, 0 failed, 54 ignored. That is 2 more than
    D-230's 1083, the two `Send` checks. The Redis TLS unset check exits 101, as O-66 expects.
  - Q-255 stays open. `WP-3-5` can now use `NonPersonalized` and the expression types across threads.
  - Verdict: accepted.

- **D-233 Design of the item-to-item and user-to-user logics (WP-3-5, the fold of D-WP35-1 to D-WP35-4).**
  1. Enums over four concrete recommenders (D-WP35-1). Go's `ItemToItem` and `UserToUser` interfaces are
     the enums `ItemToItem` and `UserToUser`. Each wraps one struct for a Go type: embedding, tags, users
     and auto for items; embedding, tags, items and auto for users. Go's unexported constructors fold into
     `new_item_to_item` and `new_user_to_user`, which dispatch on the configured type and read the IDF from
     the options. Go's interface has a closed set of four implementations, so an enum needs no trait object
     and no boxed future.
  2. `add` takes the processed item (D-WP35-2). Go's master passes `dataset.GetItems()` and
     `dataset.GetUsers()` to `Add` (`master/tasks.go` 816 to 818 and 865 to 867). The dataset's `AddItem`
     and `AddUser` run `processLabels` (`dataset/dataset.go` 198 to 225), so Go's `Add` sees processed
     labels. The port's `add` takes `DatasetItem` and `DatasetUser` for the same reason, and the master
     must do the same (O-75). The non-personalised path reads raw storage items, as D-225 records.
  3. The expression sees Go's types (D-WP35-3). The `item` variable is a `Value::Struct` of Go's
     `data.Item`, and `user` one of `data.User`. A processed label is the Go value it is in Go:
     `dataset.ID` a named `int32`, `[]dataset.ID` a slice of it, FP16 bits a `[]uint16`, and a map a
     `map[string]any`. `data.Item.UpdateAt` is not copied by `AddItem`, so the port's `UpdateAt` is the
     zero time, as in Go.
  4. Go's type switches are exact (D-WP35-4). `flatten`, `floats.FromAny` and `anyToFloat32` match exact
     types, and the port does the same. A named type such as `dataset.ID` does not convert.

- **D-234 Court acceptance for WP-3-5 on the head `65e6308` (integrator, 2026-10-28).** The branch
  `claude/wp-3-5-logics` is merged as `91a35815` on the integration head `1c4505a9`. The branch already
  contains `1c4505a`, so the merge was clean. Q-254 is closed on this branch by D-232.
  - The ports are `logics/item_to_item.go` and `user_to_user.go` @ add21b5, in `gorse-logics`. Fourteen
    unit tests mirror the Go suite methods: eight item-to-item and six user-to-user, and each module's
    `recommender_is_send_and_sync` is one of them. None is ignored.
  - The Go oracle. `conformance/tests/item_to_item_oracle.rs` runs `contracts/tools/item2itemgolden` with
    `go run`. It writes 40 seeded cases over eight kinds, and the test asserts 40. Stored vectors match bit
    for bit. Each row's neighbours match as a set, and their scores agree within 1e-6 of `max(|score|, 1)`.
    Only neighbours tied at the lowest score may differ (X-205). The test fails when the Go toolchain is
    missing. The court read both early returns in the test: one follows an equal missing-collection flag,
    and the other follows equal neighbour counts, which are asserted first. Neither skips a comparison.
  - The court's mutation check. Changing the `auto` score scale from 0.5 to 1.0 at
    `item_to_item.rs` line 331 makes `recommenders_match_go` fail (exit 101, a panic at
    `item_to_item_oracle.rs` line 405). The file was restored byte for byte.
  - The gate on the merged tree, with every URI set: fmt, clippy, rustdoc, the config and YAML goldens,
    `go vet`, the cloud and Mongo clippy runs, the vector acceptance with `--include-ignored` (skipping
    `vectors::weaviate::tests::sparse`, as D-224 records), the vector cross-read, the proxy conformance and
    the proxy store tests all exit 0. `cargo test --workspace --no-fail-fast`: TEST section 1100 passed,
    0 failed, 54 ignored. That is 15 more than D-232's 1085: 14 unit tests and the oracle. The Redis TLS
    unset check exits 101, as O-66 expects. The thread's own gate had 23 failures, all in backend suites
    without their services, and none with the URIs set.
  - Open: O-75 (the master passes processed items), O-76 (the REST work confirms nil against empty results),
    and Q-255 (`json.Number`, which the item and user label copies follow, X-204). Q-255 stays open until
    Phase 5.
  - Verdict: accepted.

- **D-235 Design of the recommend pipeline (WP-3-7, the fold of D-WP37-1 to D-WP37-3).**
  1. `Source` replaces Go's closures, and `run` takes `&mut self` (D-WP37-1). Go's `parse` returns a
     `RecommenderFunc` closure. The port returns a `Source` enum and runs it in `Recommender::run`. Go's
     `recommendItemToItem` sorts the user's feedback in place (`data.SortFeedbacks`), so the item-to-item run
     takes `&mut self` and leaves the sorted feedback behind, as Go does.
  2. The external script runs on a blocking thread (D-WP37-2). `External::new` and `pull` use
     `reqwest::blocking` and `rquickjs`, which would block or panic on the async worker. The port builds and
     runs the script inside `tokio::task::spawn_blocking`. Go's goroutine has no such limit.
  3. `Recommend` returns a `Vec`, and nil is not kept (D-WP37-3). The port cannot tell nil from empty. O-76
     records the consequence for the REST work.

- **D-236 Court acceptance for WP-3-7 on the head `60fa669b` (integrator, 2026-10-29).** The branch
  `claude/wp-3-7-logics` is merged as `f0467076` on the integration head `7c6986ea`. The merge was clean.
  - The port is `logics/recommend.go` @ add21b5 in `gorse-logics`, with nine unit tests in
    `recommend/tests.rs`: the Go suite's methods, the unknown-recommender and missing-config errors, and
    `recommender_is_send_and_sync`. None is ignored. The thread's report counts 12 for
    `cargo test -p gorse-logics recommend`, a substring filter; the gate lists nine in `recommend::tests`.
  - The Go oracle. `conformance/tests/recommend_oracle.rs` runs `go run ./contracts/tools/recommendgolden`
    itself, so no step is hand-run. The test asserts 40 cases and 120 compared runs (three per case), and
    it requires zero differences. Its largest score gap is 4.77e-7, within the 1e-6 tolerance. In repeated
    runs, 46 to 48 of the 120 runs give a nil result in Go (O-76).
  - The court's mutation check. Reversing the `lo.Every` arguments in `recommend.rs` (the bug of X-210)
    makes `recommend_matches_go` fail with 41 differences (exit 101). The file was restored byte for byte.
    The thread's own run of that mutation gave 34 differences.
  - The gate on the merged tree, with every URI set: fmt, clippy, rustdoc, the config and YAML goldens,
    `go vet`, the cloud and Mongo clippy runs, the vector acceptance with `--include-ignored` (skipping
    `vectors::weaviate::tests::sparse`, as D-224 records), the vector cross-read, the proxy conformance and
    the proxy store tests all exit 0. `cargo test --workspace --no-fail-fast`: TEST section 1110 passed,
    0 failed, 54 ignored. That is 10 more than D-234's 1100: the nine unit tests and the oracle. The Redis
    TLS unset check exits 101, as O-66 expects. The thread's 23 failures were backend suites without
    services, and none fails with the URIs set.
  - Limits, all kept open. O-30 is extended: the oracle keeps only cases whose three Go builds agree, so it
    never compares against an unstable reference, but it also leaves out the cases where Go's approximate
    answer is what a user would see. The external recommender is covered only by the ported `TestExternal`,
    since the oracle starts no script server. X-209: the ported suite misses the user-to-user digest for
    type `items`, and the oracle covers it.
  - Not touched: O-75 (`recommend` reads processed items from the stores, not from the master) and Q-255
    (the pipeline evaluates no expressions).
  - Verdict: accepted, with O-30 and O-76 open and extended. The bug X-210 was caught by the oracle and
    fixed in the code.

- **D-237 WP-4-7 waits for the SDK dataset, and nothing substitutes for it (coordinator ruling, 2026-10-29).**
  The Go SDK suite's setup (`client/setup-test.sh`) downloads `ml-100k.bin.gz` from `cdn.gorse.io`, which this
  environment blocks (O-1). WP-4-7 stays blocked until `ml-100k.bin.gz` is in `/mnt/project-files/datasets/`.
  No other dataset, stub, fixture or skip may stand in for it, and the suite is never marked ignored: a
  skipped suite is not a pass. The Phase 4 exit gate stays open under O-1 until WP-4-7 passes.

- **D-238 `cmd/goat` is not ported (coordinator ruling, 2026-10-29).** `cmd/goat` generates the assembly that
  Phase 0 replaced with C compiled through `cc` (D-25 records the clang options that `goat` gave when it
  generated Go's assembly). The tool has moved out of this tree to `github.com/gorse-io/goat` (its README in
  `cmd/goat`). No WP ports it, and the Rust port has no `goat` binary.

- **D-239 `cmd/gorse-bench` is WP-4-8, after WP-4-5 (coordinator ruling, 2026-10-29).** The benchmark tool is
  `cmd/gorse-bench/main.go` (600 lines, no Go tests). It is a Phase 4 package, so it gets its own WP and its
  own mixed-cluster check, and it does not block the node packages. The brief is `phase-4-briefs.md`.

- **D-240 The dashboard bundle is vendored and embedded, and `cargo build` needs no network (coordinator
  ruling, 2026-10-29).** The bundle is the one `master/rest.go` embeds through `github.com/gorse-io/dashboard`,
  pinned at the version the `go.mod` of the pinned commit uses: `v0.5.20260714`, with `go.sum` hash
  `h1:Sy7G7Jw1JywSl5KYDSLiIYctKHBxom4qQmIn+aS1Z1c=`. A fetch script downloads the module through the Go module
  proxy (`go mod download -json`), checks its hash against `go.sum`, and extracts the bundle into a vendored
  directory. The vendored directory is committed unchanged and embedded with `rust-embed`. The bundle is a zip
  that the module's `statik.go` registers as a string literal, not the module zip itself. WP-4-4 owns the
  script, the directory and the embedding.

## Open questions for Ritchie


- **D-241 WP-4-8 test design (coordinator ruling, 2026-10-29).** The coordinator approved three choices in the
  WP-4-8 brief. The acceptance test is `bench_golden`, not `mixed_bench`. The bench's comparison runs on a
  synthetic dataset from a fixed seed, seeded into the MySQL database named by `MYSQL_URI`, so it does not depend
  on O-1. The embedding subcommand's OpenAI call is tested against a local stub server, because the bench reads
  `cfg.OpenAI.BaseURL` (`cmd/gorse-bench/main.go:374`, config key `openai.base_url`, env `OPENAI_BASE_URL`). The
  stub reproduces the responses of `common/mock/openai.go`. No open item is recorded for this call. If the Rust
  side cannot use the stub, the thread reports why and does not skip the case.
- **D-242 WP-4-2 server node merged in part (court verdict, 2026-10-29).** `claude/wp-4-2-server` at d85bb217
  is merged `--no-ff` into integration (merge commit 821eca10) with no conflicts. The merged tree passes the
  gate with every backend URI set: fmt, clippy `-D warnings`, rustdoc `-D warnings`, the config and YAML goldens,
  `go vet`, the cloud and Mongo clippy runs, the vector acceptance and cross-read, the proxy conformance and proxy
  store tests. The test section has 1136 passed, 0 failed, 54 ignored (1110 before the merge plus 26 server
  tests). `REDIS_TLS_UNSET` exits 101, as the gate expects. The court read the diff for cheating patterns. The 23
  suite methods of `server/rest_test.go` each map to one test with the Go name in snake case, and `TestServer_Sync`
  is `server::tests::server_sync`. The `return;` statements in `server.rs` are the Go error paths of the life
  cycle, not test skips. No test is ignored, skipped or weakened. `Send` and `Sync` are checked at compile time for
  `RestServer` and `Server` (Q-254). The notes are folded as Q-265 to Q-267, X-212 to X-225, and O-76 (extended).

  The WP is **not accepted**. Before acceptance the thread must do four things. (1) Write
  `conformance/tests/mixed_server.rs` against Go master and Go worker, which needs `conformance/src/cluster.rs` from
  WP-4-1 (not yet on `origin/claude/wp-4-1-worker` at 991735c). It fails, rather than skips, when Go or a backend
  is missing. (2) Fix X-219 (case-insensitive JSON keys, which Go accepts and the port refuses) or get a coordinator
  ruling plus a test that records the refusal. (3) Delete `crates/gorse-server/src/rest/tests/visibility.rs`, a
  three-line placeholder with no test. The `visibility` test lives in `recommend.rs`. (4) Q-266 (`float_roundtrip`)
  is merged because the gate passes with it. It is a workspace feature unification, so it stays provisional until the
  coordinator approves it. A rejection is a one-line revert in `crates/gorse-server/Cargo.toml`.

- **D-243 `float_roundtrip` on `serde_json` is approved (Q-266, coordinator ruling, 2026-10-29).** `gorse-server`
  keeps the feature, and the workspace unifies it, because correctly rounded parsing matches Go's `strconv`. The
  one-ulp difference in the item-to-item test is the evidence. The merge is `864d024c`.

- **D-244 The server matches JSON keys case-insensitively (X-219, coordinator ruling, 2026-10-29).** No exemption.
  Go's decoder matches keys without regard to case, so `"itemid"` for `ItemId` must be accepted, with a test. WP-4-2
  is not accepted until that test passes.

- **D-245 Request context is `Context::background()` per request (WP-4-2, D-WP42-1).** Go passes the request
  context. axum cancels a handler when its future is dropped, which covers the common case. Propagating the request
  context is not needed for the tests.

- **D-246 Request events are emitted on `spawn_blocking` (WP-4-2, D-WP42-2).** Go emits them on a goroutine, so the
  response does not wait for the handler.

- **D-247 Tests open fresh stores per test (WP-4-2, D-WP42-3).** Go's suite shares them and purges them in
  `SetupTest`. Each Rust test sees only its own rows.

- **D-248 The router's fallback trims the trailing slash (WP-4-2, D-WP42-4).** The routes keep Go's route table
  minus the slash (X-220).

- **D-249 Per-user work runs off the runtime (WP-4-1, D-WP41-1).** `Pipeline::recommend` runs each user's job on
  `spawn_blocking` inside `parallel::detachable`. Async store calls go through `Handle::block_on`. The HTTP rankers
  detach their parallel context around the API call, so a blocking request does not hold a worker.

- **D-250 Progress is reported every 10 seconds (WP-4-1, D-WP41-2).** Progress goes through an unbounded channel to a
  reporter that pushes `encode_progress` on Go's tick. Each push is its own task. Go pushes inline, so this differs
  only in timing.

- **D-251 Stores follow `worker.go`'s sync (WP-4-1, D-WP41-3).** The worker connects to `sqlite://` data and cache
  stores and to `xvec://` vector stores through the master's proxies. Other URIs are opened directly.

- **D-252 Metrics are registered at start-up (WP-4-1, D-WP41-4).** `Worker::new` calls `metrics::register()`, which
  forces every `LazyLock`, so `/metrics` lists the worker's gauges from start-up, as Go's `promauto` does.

- **D-253 The worker reports the crate version (WP-4-1, D-WP41-5).** `CARGO_PKG_VERSION` stands in for Go's ldflags
  version.

- **D-254 A config update is applied as a clone (WP-4-1, D-WP41-6).** The master's config is unmarshalled into a clone
  and then set. A failed unmarshal keeps the old config.

- **D-255 The harness edits TOML by line (WP-4-1, D-WP41-7).** No TOML crate is in `Cargo.lock`, so `set_key` edits one
  key's line in one section of `contracts/config/config.toml`, and appends a missing key or section. A contract test
  checks that every key the harness sets exists in the contract file.

- **D-256 The harness builds three Go binaries (WP-4-1, D-WP41-8).** `go build` builds `gorse-master`, `gorse-server`
  and `gorse-worker`. It does not build `gorse-cli`, which no node needs. The brief's command line includes it, so
  the brief should drop it.

- **D-257 The Rust worker runs in the test process (WP-4-1, D-WP41-9).** It runs through `gorse_worker::Worker`, and no
  binary is built for it. Its readiness and shutdown use the same HTTP and stop path as the Go worker.

- **D-258 The mixed-test config is narrowed (WP-4-1, D-WP41-10; requirement in D-262).** The test sets
  `recommend.collaborative.type = "none"`, `recommend.ranker.type = "none"` and `recommenders = ["latest"]` in the
  ranker and the fallback. The reason given is that matrix factorisation and FM training differ between two runs for
  reasons unrelated to the port. Go's MF training runs parallel updates over users and items
  (`model/cf/model.go` 448, 659, 707), so the reason is plausible. It is not yet shown by a run, and the narrowing is
  not accepted until it is (D-262).

- **D-259 The mixed-test data is fixed (WP-4-1, D-WP41-11).** Ten users, thirty items one minute apart in categories
  `i % 3`, and two `like` feedbacks per user. The timestamps are fixed, so the latest recommender orders items alike
  in both runs.

- **D-260 The mixed comparison is exact (WP-4-1, D-WP41-12).** Rows are compared as sorted lists of (item id, score
  bits, categories), so the comparison is over sets with multiplicity. Digests are compared exactly. The test has no
  dense-vector results, so the stable-case rule does not apply.

- **D-261 The worker's shared state is checked for `Send` and `Sync` (WP-4-1, D-WP41-13, Q-254).** `lib.rs` checks that
  `Pipeline`, `ItemCache`, `CachedItem`, `Worker`, `WorkerOptions` and `HealthStatus` are `Send + Sync`. WP-4-3 adds
  the master's check.

- **D-262 WP-4-1 worker node merged in part (court verdict, 2026-10-29).** `claude/wp-4-1-worker` at a50ea783 is
  merged as one unit (`79a427ff`): `gorse-worker`, `conformance/src/cluster.rs`, `conformance/tests/mixed_worker.rs`,
  the `gorse-worker` dependency in `conformance/Cargo.toml`, and the tokio features. The workspace keeps one
  `prometheus` entry with default features, since the worker adds `process`. `Cargo.lock` is regenerated, with four
  packages added and no existing version changed. The gate on the final tree (`f74c559b`, with the WP-4-2 update) passes
  with every backend URI set: the test section has 1183 passed, 0 failed, 55 ignored, and `mixed_worker` passes against
  the Go master and Go worker. The court's mutation check drops the last cache row per user in `pipeline.rs` before
  `add_scores`. `mixed_worker` then fails with exit 101 and reports the rows that only Go writes, for every user. The
  file is restored byte for byte. The two digests agree in that run while the rows differ, so the row comparison is what
  catches a cache-row difference; the digest does not cover the cache write.

  The gate needed three environment fixes, recorded as O-79 and O-83. The worker's `gorse-storage` features (redis, s3,
  gcs, azure, mongodb, qdrant, milvus, weaviate) are on in the workspace test run, so the feature-gated store tests now
  run in the default gate and need their environment. The S3 and Azure tests need the emulator variables and the
  `gorse-test` bucket and container. The cross-read tests leave Qdrant collections behind (O-83).

  The worker is **not accepted**. Before acceptance, the thread must do four things. (1) Wire TLS to the master through
  `gorse_common::util::tls`, since the contract requires it when all three `--ssl-*` flags are set (X-236, O-78). (2)
  Show, by two Go runs with `jobs` above 1 under `collaborative.type = "mf"`, that Go's CF output differs between runs.
  If it does not, enable CF in `mixed_worker` (D-258, O-80, Q-268). (3) Check the Redis and Mongo `add_scores` on an
  empty slice, and record the answer for O-76 (X-233). (4) Check the reranker template rendering of a numeric array
  against Go's output, with a test that builds Go itself (O-77). O-81 (no empty-list case) is tracked and does not block.

- **D-263 WP-4-2 server update merged (court verdict, 2026-10-29).** `claude/wp-4-2-server` at 3bcb7d2 is merged as
  `f74c559b`. JSON member names match without regard to case, as Go's decoder matches them (X-219, D-244). Five tests in
  `rest/tests/names.rs` cover it, and four of them fail when folding is disabled. The Go expectations were checked
  against a Go `json.Decoder` with `UseNumber`. The placeholder `rest/tests/visibility.rs` is deleted. `float_roundtrip`
  is on the workspace `serde_json` (D-243). The four `D-WP42-n` decisions are folded as D-245 to D-248. The gate result is
  in D-262. **WP-4-2 is still not accepted.** `mixed_server.rs` is not on the branch. Its draft is at
  `/mnt/project-files/reports/wp-4-2/mixed_server.rs.draft`, and the court has not reviewed it. It lands on
  `claude/wp-4-2-server` with the per-route result in the section 8 report.

- **D-264 WP-4-1 worker node accepted (court verdict, round 2, 2026-10-29).** `claude/wp-4-1-worker` at `52977e9` is
  merged as `ef046790`. The four court items from D-262 are answered, and the court checked each one.
  1. TLS to the master (`ee04534`, X-236, O-78): the worker builds its client config with `gorse_common::util::tls`, as
  Go's `util.NewClientCreds` does. Go uses TLS only for the master connection (`worker/worker.go` 399), and its HTTP
  server is plain (`ListenAndServe`, line 333), so the port matches. The mutual-TLS mixed test passes, and it fails when
  the worker logs a master-sync error.
  2. The collaborative-filtering check (`35094c7`, D-258, O-80): the court ran
  `contracts/tools/cfgolden -model BPR -jobs 4 -runs 3` and got three different digests (`af5bb652`, `d55486b7`,
  `d5203316`). That supports keeping collaborative filtering off the mixed test. The thread reports one ALS digest at both
  job counts; the court has not run ALS itself.
  3. Redis `add_scores` with no documents (`2773cc4`, X-233, O-76): Go sends nothing and returns nil, even with a
  cancelled context, and the Rust port now matches. Mongo already matched. O-76 stays open for the REST routes (WP-4-4).
  4. The reranker template check (`2bc69e7`, O-77): nine cases checked against gonja with `go run`, and a formatter
  matches gonja's float rule. The RED failure was real, and the thread fixed the port rather than the test.

  The gate on the merged tree (`ef046790`) passes with every backend URI set: the test section has 1186 passed, 0 failed,
  55 ignored, and the two mixed-worker tests, the reranker test and the Redis test all pass. The `REDIS_TLS_UNSET` check
  exits 101, as expected. The court cleared six leftover `/tmp/.tmp*` Go build directories (388 MB each), none in use;
  the thread had reported five.

  Open, and not blocking: Q-268 (should `mixed_worker` also run with `collaborative.type = "mf"`; ALS and BPR must first be
  shown deterministic), O-80 (the latest recommender is the only one compared across nodes), O-81 (no empty-list case in
  the mixed run), and O-84 (Go panics on a template list that mixes bool, string and nil, so that output is not compared).

- **D-265 WP-4-2 server node merged in part, not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server` at
  `fff5c5d` is merged as `8261d2ee`. The head moved from `86d979b` to `fff5c5d` during review (one comment, 258 to 256
  rows), so the gate ran on `fff5c5d`. The court re-ran the Go decode probe: `go run .` on `corpus4.jsonl` reproduces
  `oracle4.jsonl` byte for byte, and `make_fixture.py` reproduces `go_decode.jsonl` byte for byte.

  Accepted. The JSON error texts for non-EOF syntax, depth, type and time errors, and the content-type gate (X-238), with
  the probe's rows as evidence. The master config decode fix (`8959c39`): the poll interval had been zero, so the loop
  never slept and no store was opened, and `server_sync` had hidden it with snake_case input. The Go float text, the
  truncated-body texts and the `HOSTNAME` removal (X-225). The mixed cluster (63 requests over all 42 routes, 0
  differences) is evidence for the routes it covers.

  Not accepted. The coordinator asked the court to judge X-WP42-16 to 19 against Go. The court ran the probe and read the
  Go source. These four are behaviour gaps, not deviations, and they block acceptance:
  - X-240 (labels): Go accepts `"Labels":[1e400]` on `user` and `item`, and a label 130 arrays deep, and keeps the number
    as `1e400` (probe run). The port answers serde's texts. The thread's pinned test records the gap, and does not close it.
  - X-241 (XML): go-restful registers `encoding/xml` for `application/xml` by default, and the server's routes set only
    `Produces` (`server/rest.go` 232 to 236). Go decodes `<a/>` on `user` to the empty user (probe run). The port refuses
    it. The fixture filter drops that row.
  - X-242 (Content-Encoding): go-restful's `ReadEntity` inflates `gzip` and `deflate` bodies on every route, and
    `EnableContentEncoding` does not control it (`request.go` 75 to 89, v3.13.0; read from source, not run live).
  - X-243 (quoting): Go escapes an unassigned code point in an error text (probe run: `'\u0378'`), and the port quotes it
    as itself. Narrow, but it is an error text, and the rule is exact text.

  X-239 (the host name read order) is not a gap on Linux and stays provisional.

  Also required: O-85 (the decode oracle runs by hand and `make_fixture.py` drops 12 of 268 rows, including the XML row
  that Go decodes; the probe must run from the conformance crate, as the other oracles do) and O-86 (no live mixed case for
  XML, compressed or large-number bodies).

  The gate on the merged tree (`8261d2ee`, every backend URI set): The gate on the merged tree (`8261d2ee`, every backend URI set; the server head `6ca5a43` differs from it only by `porting/notes/WP-4-2.md`, which is folded here and removed; recorded by merge `3f9b6b2b`): the test section has 1193 passed, 0 failed, 55 ignored across 89 suites, and every stage exits 0 except `REDIS_TLS_UNSET`, which exits 101 as the negative check requires (its two failures are the unset `REDIS_TLS_URI`). The mixed cluster, the decode probe rows, the Mongo cursor decode and the hostname test all pass. The court did not mutation-check the decoder this round. The server thread reports one run at `c71dd83` that exited 101 with the disk full and no output, and two reruns passed. Our gate passed, and the cause is unexplained; it is not blocking..

- **D-266 WP-4-2 server node merged in part, still not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server` at
  `bb170ce` is merged as `6b2be383`. Code is at `5440113`; `bb170ce` changes only the notes. The gate on the merged tree,
  every backend URI set: the test section has 92 suites, 1199 passed, 0 failed, 55 ignored. Every stage exits 0 except
  `REDIS_TLS_UNSET`, which exits 101 as the negative check requires. `go_decode`, `go_printable` and `go_xmlname` each run
  Go at test time and pass. `mixed_server` has 83 requests over the 42 routes, with 0 differences.

  Closed, with evidence in the code and the gate. X-240 labels keep their number text, and nesting to 10000 levels is decoded
  and validated without recursion (`3f11050`). X-241 XML bodies are decoded as `encoding/xml` decodes them (`81826a6`). X-242
  gzip and deflate bodies are inflated as go-restful inflates them (`e5fbec4`). X-243 quoting follows Go's `unicode.IsPrint`
  (`5ff737a`). O-85: `go_decode` is the only decode oracle and runs all 268 corpus rows against Go at test time; the stored
  fixture and the filter are removed. O-86: the mixed cluster covers XML, gzip, deflate and large labels (83 cases). The
  `go_decode` change was checked live: a deliberate change to one entity fails exactly the row that uses it (the thread's
  check; the court did not repeat it). Q-267 is answered: Go's storage also reads `2.50` back as `2.5`, and the mixed test's
  `item-num2` case matches. The `1e400` labels answer `500` with the storage RPC's text on both clusters, which is also Go's.

  Still open, and it blocks acceptance. X-244 (WP-4-2, X-WP42-20): the storage backends walk a label recursively, so a
  Go-valid label of 10000 levels can exhaust a 2 MB thread stack. The court reproduced it on the serde path with a probe
  (`Labels::Array` nested 10000 deep, `serde_json::to_string`, a 2 MB thread): a debug build aborts with a stack overflow,
  and a release build passes at 10000 and overflows at 18000 levels. The server's own frames sit on top of that stack, so
  the production margin is smaller than the probe shows. The Mongo `labels_to_bson` path was not measured. The probe is not
  committed. X-239 (host name read order) stays provisional and does not block.

  Acceptance needs X-244 closed: either the storage walks become iterative, or the runtime runs storage calls on threads with
  a larger stack (WP-4-5 owns the binary's runtime). Either way, a test shows a 10000-level label round-trips through SQL and
  Mongo on the Rust server in a release build. The coordinator assigns the owner (D-266).

- **D-267 Coordinator ruling on X-244: iterative label walks in storage (2026-10-29).** Go's goroutine stacks grow, so Go
  has no depth limit short of memory, and a larger fixed stack would only move the cliff. Option B (a larger runtime stack
  in WP-4-5) is therefore not taken. The label walks in the storage crates must use an explicit stack on every backend: SQL,
  proxy, and Mongo's `labels_to_bson`. The WP-4-2 server thread owns the change and may edit only the label-walk functions in
  the storage crates. Done when: debug-build tests at 10,000 and 100,000 levels pass on every backend path; existing test
  outputs are unchanged; and the integrator runs the full gate with every backend URI on the merged head. WP-4-2 acceptance
  waits for it (D-266). **Amendment (coordinator, 2026-10-29).** Go's `jsontext` caps nesting at 10,000 levels (`errMaxDepth`), so the port
  reproduces that cap with Go's error text: 10,000 levels succeed, and 100,000 are rejected exactly as Go rejects them.
  The indentation fix in `gorse-server`'s `rest/response.rs` is approved as part of X-244: go-restful pretty-prints every
  JSON response by default (`PrettyPrintResponses = true`, `MarshalIndent` with one space, go-restful v3.13.0), and
  the server does not turn that off, so a label written compactly inside a response is a mismatch. Mongo's depth is
  checked by the live Go-versus-Rust conformance test, which the integrator runs.

- **D-268 WP-4-2 X-244 head merged in part, still not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server` at
  `250c853` is merged as `4292562d`. Code is at `ca175c9`. The gate on the merged tree with every backend URI set: 92
  suites, 1210 passed, 0 failed, 55 ignored in the TEST section. Every stage exits 0 except `REDIS_TLS_UNSET` (101, the
  negative check) and `MIXEDMONGO` (101, below). The SQL and proxy label paths pass at 10,000 levels in debug, and the
  REST changes pass `mixed_server` (83 requests, 0 differences).

  Not accepted. `mixed_mongo_deep_labels_match_go_server` aborts in a debug build with a stack overflow while it seeds the
  Rust store. A gdb backtrace shows the recursion is in the `bson` crate's raw serializer (`Bson` through
  `serialize_value` and `serialize_entry`, about 540 bytes of stack per level, so about 5 MB at 10,000 levels). The storage
  walks are iterative, but the Mongo write path still hands a nested `Bson` tree to the driver, which serializes it
  recursively. The read path and `Bson`'s own drop are recursive too, and the server thread's `drop_bson_iteratively` is
  a test helper, not the production path. This is the case D-267 requires to pass, so X-244 stays open.

  Two environment notes. (1) The test builds `{MONGO_URI}/gorse_mixed_go_<run>`, so it cannot carry `?authSource=admin`.
  Against the court's authenticated Mongo it fails at authentication, which is why the court ran it on an unauthenticated
  `mongo:6` container on port 27018, which was then stopped. The test should take a URI that can carry a query, as the
  other Mongo tests do. (2) The question in the thread's notes, Q-WP42-3, was answered in D-266: Go's storage also reads
  `2.50` back as `2.5`, so the port matches Go and the text is not kept on either side.

  Needed before acceptance: the Mongo write and read paths for labels must not recurse. The server thread should build and
  read raw BSON with an explicit stack. Scope is a question for the coordinator, because the change reaches the document
  types and the insert and find calls, not only the label-walk functions D-267 names.

- **D-269 Coordinator ruling: the Mongo raw-BSON fix is in scope for X-244 (2026-10-29).** The integrator recommended it
  and the coordinator agreed: the Mongo write and read paths for labels must build and read raw BSON with an explicit stack,
  not a nested `Bson` tree. This reaches the document types and the insert and find calls, not only the label-walk
  functions that D-267 named, and the server thread may make that change under this ruling. Done when: a live 10,000-level
  debug test on the Mongo path matches Go's outcome, and the existing outputs are unchanged. WP-4-2 acceptance waits for
  both, and the court runs the full gate on the merged head (D-268).

- **D-270 WP-4-2 Mongo raw-BSON head merged in part, still not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server`
  at `446ab86` (code at `6836098`) is merged as `a070c2a0`. The gate on the merged tree with every backend URI set: the
  test section has 92 suites, 1217 passed, 0 failed, 55 ignored. Every stage exits 0 except `REDIS_TLS_UNSET` (101, the
  negative check) and `MIXEDMONGO` (101, below). The Mongo deep-label test runs to completion now, in 3.8 s, with no stack
  overflow, so the D-268 abort is gone.

  Not accepted. `mixed_mongo_deep_labels_match_go_server` reports one route that differs: `POST /api/user`, the write that
  reaches MongoDB at 9,999 levels (10,001 is refused at the REST layer on both sides, which is an inference from the thread's
  mixed cases). Both sides get MongoDB's `(Overflow) BSONObj exceeds maximum nested object depth` error, but they name
  different fields. Go: `'u.$set.labels.a.a…'`. Rust: `'updates.0.u.$set.labels.a.a…'`. Go writes users with `BulkWrite` and
  `UpdateOneModel` (`storage/data/mongodb.go` 617 to 622), and the Rust side sends a raw `update` command, whose element path
  carries `updates.0.`. The text must match exactly, so X-244 stays open, and it blocks acceptance. The server thread
  reproduces the path Go's driver reports, and the live test is the oracle. The court answered the thread's open question:
  MongoDB's depth refusal does not match Go's wording today.

  Environment. The live test leaves `gorse_mixed_go_*` and `gorse_mixed_rust_*` databases on the court's Mongo. Nothing drops
  them, and they are to be dropped at the next court run.

- **D-271 WP-4-2 exact-prefix head merged in part, still not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server`
  at `dd0a3cc` (code at `133a7cd`, the exact-prefix strip) is merged as `04a31b9f`. The gate on the merged tree: FMT,
  CLIPPY, DOC, CONFIGGOLDEN, YAMLGOLDEN, VET, CLOUDCLIPPY and MONGOCLIPPY exit 0. The TEST section has 1220 passed, 0
  failed, 55 ignored, with the Mongo deep-label test skipped there as the gate requires. `MIXEDMONGO` exits 101:
  `mixed_mongo_deep_labels_match_go_server` still reports one route that differs, `POST /api/user`. Both replies now begin
  the same way, `(Overflow) BSONObj exceeds maximum nested object depth in element with field name 'u.$set.labels.a.a…`,
  so the exact-prefix strip takes effect; the difference lies further along the body. The failure report cuts each body
  at 100 characters, so the full bodies are not in the log yet.

  Not accepted. X-244 and X-WP42-20 stay open, and so does the acceptance gate. The run stopped during the vector
  acceptance stage when the Docker daemon died (the log stops at 06:41 UTC). The stages after MONGOCLIPPY have not run on
  `04a31b9f`: vector acceptance, REDIS_TLS_UNSET (101 is expected), vector cross-read, proxy conformance and proxy store.
  A full gate run from FMT is needed once the live test passes.

  Deviation, logged as the coordinator ruled (D-269): the strip removes the exact leading `updates.0.` from the first
  field path, and only there. Unit tests cover it. The coordinator's condition holds: the live test must pass before
  acceptance.

  Next step: restore Docker, capture both full reply bodies from the live run (a local, uncommitted print in the test
  helper is fine), and send them to the server thread. The notes for `dd0a3cc` are at
  `/mnt/project-files/handoff/wp42-notes-dd0a3cc.md`; they are not yet folded into PORTING_NOTES.md.

- **D-272 Coordinator ruling on X-244: a vendored `mongodb` 3.9.1 with a document-sequence patch (2026-10-29).** The
  prefix strip is replaced. `mongodb` 3.9.1 has no public way to send an update statement as an OP_MSG document sequence
  (`/mnt/project-files/reports/wp-4-2/update-document-sequence.md`). The server thread vendors 3.9.1 unchanged in
  `third_party/mongodb-3.9.1`, applies the patch that sends `updates` as payload type 1, as Go's driver does, and adds
  `UpdateModifications::Raw` so labels reach the driver as raw bytes. It is wired in with `[patch.crates-io]`, and the strip
  and the `update` command builder are removed (`bd00d75`). The deviation is C-34. Acceptance needs the live
  `mixed_mongo_deep_labels_match_go_server` to match Go byte for byte on the full gate. Checked by the court: the patch is 69
  lines in two files, and the vendored tree matches the registry copy apart from those two files, `GORSE.md` and `LICENSE`.

- **D-273 WP-4-2 MongoDB head `321946c5` not accepted (court verdict, 2026-10-29).** `claude/wp-4-2-server` at `dd0a3cc` is
  merged as `04a31b9f` (D-271) and was recorded at `321946c5`. The court ran the gate with every backend URI set. The live
  `mixed_mongo_deep_labels_match_go_server` fails on one route, `POST /api/user`. The full bodies are in
  `/mnt/project-files/handoff/mixed-mongo-post-api-user-bodies-321946c5.txt`. Go's name has 201 path elements (`u`, `$set`,
  `labels`, 198 `a`). Rust's name has 199 after the strip (`labels`, 196 `a`), and 201 before it, because `updates` and `0`
  are two more. Both end `' in object with unknown _id`, and neither is cut short, so the length-cap reading does not fit. The
  two levels are the depth of the statement under `updates` and `0`: Mongo refuses both at the same absolute depth, counted
  from its own top-level document, and the inline command makes that document two levels higher. A prefix strip cannot close
  it, so D-272 replaces the strip. Gate on this head: FMT, CLIPPY, DOC, goldens, `go vet`, cloud and Mongo clippy, vector
  acceptance, the cross-read, and proxy conformance and store all exit 0; `REDIS_TLS_UNSET` exits 101, the negative check. The
  TEST section has 92 suites, 1217 passed, 2 failed and 55 ignored. The two failures are `cache::redis::tests::escape_characters`
  and `recommend_matches_go`. They are recorded as O-87 (D-274). The mixed-cluster tests and the `go_decode` and `go_printable`
  oracles pass in that run.

- **D-274 WP-4-2 server node accepted, X-244 closed (court verdict, 2026-10-29).** `claude/wp-4-2-server` at `7a0b394` (code
  `bd00d75`) is merged as `84e7b98`. The gate on the merged tree, with every backend URI set and `MONGO_URI` carrying
  `?authSource=admin` for the live test:
  - FMT, CLIPPY (`--workspace --all-targets -D warnings`), rustdoc with `-D warnings`, the config and YAML goldens, `go vet`,
    cloud clippy and Mongo clippy: exit 0.
  - TEST section: 92 suites, 1217 passed, 0 failed, 55 ignored. The count is from the test section only.
  - `mixed_mongo_deep_labels_match_go_server`: 1 passed in 4.71 s. No route differs, so the Go and Rust nodes answer the same
    bytes for the labels at 9,999 and 10,001 levels, on a live MongoDB 6.
  - Vector acceptance (`--include-ignored`, Qdrant, Milvus and Weaviate), the cross-read, proxy conformance and proxy store:
    exit 0.
  - `REDIS_TLS_UNSET`: exit 101, the negative check.
  - Deep labels on each backend path, in debug: the JSON round trip at 10,000 levels and the refusal at 100,000 (`data::types`);
    the MongoDB conversion both ways at 10,000 and 100,000 (`data::mongodb`); the protobuf round trip at 10,000 and 100,000
    (`data::proxy`). The SQL store writes labels as JSON, which the `data::types` tests cover.
  - The patch (C-34) was checked against the registry copy: two files differ, both with the reason in `GORSE.md`.
  X-244 is closed by the live test and the patch. The strip and its tests are gone. The open items are X-239 (provisional, the
  host name read order, not blocking), Q-265 (the neighbour `offset` past the end, not covered by the mixed test) and O-87.
  The court accepts with O-87 open: the two TEST failures on `321946c5` pass when run alone (`escape_characters` 3 of 3,
  `recommend_matches_go` 2 of 2) and both pass in the full run on this head. Neither is WP-4-2 code, and the court did not
  weaken either test. The earlier gate run on the same head passed the TEST section (`gate60`). Both tests must be made
  independent of each other and of the wall clock before the Phase 4 exit gate.

- **D-275 WP-4-3 master core accepted, O-75 closed (court verdict, 2026-10-29).** `claude/project-thread-4aex1f` at `b849d11` (one commit on the WIP commits `aede361` and `52aa0bb`) is merged as `21ca977`. The gate on the merged tree, with every backend URI set and `MONGO_URI` carrying `?authSource=admin` for the live test:
  - FMT, CLIPPY, rustdoc, the config and YAML goldens, `go vet`, cloud clippy, Mongo clippy, vector acceptance, the cross-read, and proxy conformance and store: exit 0.
  - `mixed_mongo_deep_labels_match_go_server`: exit 0. `REDIS_TLS_UNSET`: exit 101, the negative check.
  - TEST section: 1235 passed, 1 failed, 55 ignored. The count is from the test section only. The one failure is `cache::redis::tests::escape_characters` (the store returned no documents, `redis.rs:1027`). It is the O-87 class, and this merge changes no storage crate. On the court's rerun it passed alone 10 of 10. The court accepts with O-87 open, the same ruling as D-274. This is a ruling on the gate's TEST result, and the coordinator may overrule it.
  - WP-4-3 tests in that run, all passing: `rpc::tests::{test_rpc, test_reload_config, test_ssl}`, the 12 `tasks::tests`, `metrics::tests::{metric_names_match_go, test_online_evaluator}`, and both `mixed_master` tests (`rust_master_matches_go_master_in_a_mixed_cluster`, `rust_master_trains_the_go_model_in_a_mixed_cluster`).
  - Checked beyond the gate. The O-75 test (`tags_recommender_reads_processed_labels_only`) and the X-199 order test are present and pass. The changes outside the WP's crates are needed by the master and are accepted: serde on the `gorse-model` CTR and CF `Score` types, with Go's field names (`NDCG`, `Precision`, `Recall`, `RMSE`, `Accuracy`, `AUC`), because Go's structs have no JSON tags; `Model: Send + Sync`; and `Clone` on `gorse-dataset`'s `Dataset` and `Labels`.
  - The 16 deviations are in `porting/notes/WP-4-3.md`. The court rules as follows. Deviation 15 (the blob check decodes the entries and compares them exactly, because Go's `MatrixFactorizationUsers.Marshal` writes in map order) is accepted, since the test first checks that two Go runs agree. Deviation 16 (the ERROR-line allowlist) is accepted, since it matches the one line Go logs (`logics/item_to_item.go:157`). Deviation 2 (the labels and FP16 view) is accepted on the mixed-cluster evidence, where rows and embeddings equal Go's. Deviations 3 to 6, 8, and 10 to 14 are accepted as recorded.
  - Deviation 9 is a parity defect in the storage crate. Go's `BatchInsertItems` accepted 12,000 items in one call (the court's probe, 0.5 s), so the Rust rejection is not Go behaviour. The master's production code does not batch-insert, so this WP is not blocked. It is logged as X-245 for the storage owner, to be fixed before the Phase 4 exit. The court did not reproduce the Rust failure itself.
  - Deviation 1: the master does not set the dataset gauges that Go sets in `master/tasks.go` 159 to 214. The brief requires the names and label sets, not the values. It is logged as O-88, open and low.
  Open after this verdict: O-87 (`escape_characters` and `recommend_matches_go`, to be made independent before the Phase 4 exit), X-245 (assigned to the O-87 thread), O-88 (owner to be set by the coordinator, the court recommends WP-4-4), X-239 (provisional), Q-265, O-76 (WP-4-4), and O-1 (datasets, WP-4-7). The coordinator requires the Phase 4 exit gate to be fully clean.
- **D-276 O-88 closed: the dataset gauges and the memory gauges are set and pinned (court verdict, 2026-10-29).** The coordinator
  assigned O-88 to the WP-4-3 thread. Two commits on top of the accepted WP-4-3 head: `57fa4c5` pins the eleven gauges of
  `master/tasks.go` 159 to 214 with `test_load_dataset_sets_gauges`. The production code already sets them in
  `load_dataset_in`, so that commit is test-only. `c30cba6` sets `MemoryInUseBytesVec` for the four splits of `master/tasks.go`
  234 to 245, with Go's labels, from the port's `DeepSize`. The values cannot equal Go's `sizeof.DeepSize` (Go walks its own
  layout and slice capacities), so this is logged as WP-4-3 deviation 1. The court checked:
  - The eleven gauges match Go's names, values and order. The label rule in the test (a label enters the index at its second
    occurrence) matches Go's `master/tasks.go` 360 to 361 and 422 to 423.
  - The memory test `test_load_dataset_sets_memory_gauges` checks that each of the four labels is above zero, that it measures
    the split its label names, and that each value grows when the dataset grows (20 users, 20 items and 40 feedbacks added).
    Both mutation checks by the thread failed as intended.
  - The merge is `d0bef1c` on both integration branches. Gate on `d0bef1c`, every URI set: FMT, CLIPPY, rustdoc, the config and
    YAML goldens, `go vet`, cloud clippy, Mongo clippy, vector acceptance, `REDIS_TLS_UNSET` (101, the negative check), the
    cross-read, and proxy conformance and store: exit 0 (the negative check excepted). `mixed_mongo_deep_labels_match_go_server`:
    exit 0. TEST section: 1238 passed, 0 failed, 55 ignored, 93 suites. The `escape_characters` flake did not recur on this run.
  - Environment note. The first gate run on `d0bef1c` reported 27 TEST failures, all from connection refused on the backend
    ports (Docker had stopped; the same happened once before in this session). The court restarted Docker and the backends and
    reran the whole gate. The rerun is the only result this verdict relies on.
  Open after this verdict: O-87 (both flakes, still to be made independent of each other and of the wall clock before the Phase 4
  exit), X-245 (the SQLite bulk insert, with the O-87 thread), X-239 (provisional), Q-265, O-76 (WP-4-4), and O-1 (WP-4-7).
- **D-277 O-87 partly accepted: the sparse-score fix and the items part of X-245 accepted, O-87 stays open (court verdict, 2026-10-29). X-246 is withdrawn by D-278.** The O-87 thread's head `3a85214` (branch `claude/o-87-flakes`, based on `b5e8066`) is merged into the integration branch as `67378b7`, after `610e919`. The changes:
  - X-246, `cache::redis` `update_scores`: the matching keys are read in two queries (a `LIMIT 0 0` count, then one `NOCONTENT` read without `SORTBY`), not pages. Go's `UpdateScores` pages the same way and loses keys in the same kind of trial (83 of 1,000 in the thread's probe; the court has not rerun that Go probe). The regression test `update_scores_with_tied_scores_on_fresh_indexes` passes.
  - Sparse scores: `sparse_dot` sums in f64 and rounds once, as Go's `SparseInnerProduct` does. `sparse_dot_matches_go_bits` compares 1,013 scores bit for bit against a Go-generated fixture. It passes.
  - X-245, items: `batch_insert_items` writes 1,000-row statements, as GORM's `CreateBatchSize` does (`storage/scheme.go:176`, checked by the court). `test_sqlite_batch_insert_12000_items` passes.
  - Gate on `67378b7`, every URI set, with Docker and the backends up: all stages exit 0 except `REDIS_TLS_UNSET` (101, the expected negative check). The live mixed-Mongo test passes. The TEST section has 1240 passed, 1 failed, 55 ignored. The failure is `cache::redis::tests::escape_characters` (the sorted search returned no documents for `c-3`, `redis.rs:1047`). It is the same flake as before, and it passes alone 10 of 10. The thread's open item 4 names it: `search_scores` still uses Go's sorted query, and the X-246 fix does not cover it. `recommend_matches_go` passed in this run.
  - The first gate run on `67378b7` failed 29 TEST tests, all from backend connection refusals, because Docker had stopped (the third time this session). The court restarted Docker detached and the backends and reran the whole gate. Only the rerun counts. The first run's log is kept in the scratchpad.
  Still open, so O-87 stays open: (a) the fresh-index sorted search in `escape_characters` (above; `search_scores` needs a fix that keeps the score order, and Go has the same exposure, so the coordinator decides the scope); (b) the tie policy of the recommend oracle (X-247, the coordinator's ruling; the thread measured about 1 failing run in 7 on this head); (c) the shared cache-suite timing flake (`scan_scores < 9`, the thread's open item 2); (d) the case-08 mechanism, not proven; (e) X-248, users and feedback batch inserts.
  The Phase 4 exit gate stays open until O-87 and X-248 close.
- **D-278 Coordinator rulings on X-247 and the fresh-index search, X-248 assigned, X-246 acceptance withdrawn (court verdict, 2026-10-29).** The coordinator ruled at 10:12 on three points, recorded here.

  1. X-247, option (a), with conditions. Only items whose `f32` scores are bit-identical form a tie group. Inside a group the items are compared as a set. Scores, and the order between groups, stay exact. At the top-K boundary, the Rust items must be a subset of the Go group, with the same count. The O-87 thread adds an automated conformance test that shows Go's tie order varies on identical input. Option (b) is rejected.
  2. Fresh-index sorted search (`search_scores`, `delete_scores`, and `escape_characters`): production keeps Go's query with no wait added, because Go shares the exposure. In the tests, the fixture that creates an index waits until `FT.INFO` reports `percent_indexed` 1 and `indexing` 0 before the test acts. That is a precondition, not a weaker assertion. Logged as X-249.
  3. X-248 (users and feedback batch inserts) is assigned to the O-87 thread.

  **Court check of the X-246 claim.** The coordinator's rule of 08:13 allows the single-query change in `update_scores` only if Go is shown to fail the same way. D-277 accepted X-246 on the thread's figure that Go loses a key in 83 of 1,000 trials. The court ran Go's own `UpdateScores` (`storage/cache/redis.go:320-365`, unchanged, `maxSearchResults` 2 as in `TestUpdateScoresWithPaginationAndTiedScores`) against the same Redis, in a throwaway test that was removed from the tree. Each trial adds five tied documents, runs one update, and reads each hash's `categories` field. Go lost no update in 1,000 trials that dropped one index with `DD` each time, and none in 1,000 trials with a unique index per trial: 0 of 2,000. The probe and logs are in `/mnt/project-files/o87-evidence/court-go-probe/`.

  The rule's condition is not met, so the X-246 part of D-277 is withdrawn. The court does not change the code, which the O-87 thread owns. The thread must show what the paged Rust loop does differently from Go (its own probe lost 68 of 1,000 with the paged loop), then either fix that difference or show Go failing in the same shape. Until then the single-query change is not accepted. The sparse-score fix and the items part of X-245 are not affected. This verdict changes no code, so it needs no gate run.

- **D-280 WP-4-4 master REST and dashboard accepted, O-76 closed (court verdict, 2026-10-29).** `claude/project-thread-lryfg0` at `8f36831` is merged into the integration branch as `c043f7f`. Its follow-up `4975ef2` (the external null list and the empty tasks list, D-279) is merged as `bb2b5a0`. Both integration branches carry `bb2b5a0`.

  Gate on `bb2b5a0`, with every backend URI set and `?authSource=admin` on the live MIXEDMONGO test: every stage exits 0 except REDIS_TLS_UNSET, which exits 101 as required. TEST: 1276 passed, 0 failed, 55 ignored. The live `mixed_mongo_deep_labels_match_go_server` passes. The WP-4-4 tests pass: `master_rest_matches_go`, `empty_cluster_matches_go`, `null_external_matches_go`, `empty_tracer_list_is_null_in_go`, `get_tasks_is_null_without_tasks`, and the `external::tests` null cases. The two panics in the log are the REDIS_TLS_UNSET negative check (`tls_verifies_the_server_by_default` and `tls_skip_verify_connects_to_a_self_signed_server` without `REDIS_TLS_URI`), which is the expected 101.

  Reviewed by reading the diff: no `#[ignore]`, `#[allow]` or Go skip is added. The one removed assertion, in `external/tests.rs`, changes `vec![...]` to `Some(vec![...])` for the new return type. `null_external_matches_go` asserts Go's `null` before it compares, and the thread's mutation check (the master forced to answer `[]`) makes it fail.

  Scope, as the thread states it: the interop runs the Go master and the Rust router on one seeded store. A Go server and a Go worker beside the Rust master (the brief's step 6) are not run. That is open as X-251.

  O-76 is closed for the master routes: cluster, timeseries, the make-built lists, user-to-user past the end, external (D-279) and tasks (the empty-list test pins Go's `Tracer.List` as `null`). The OIDC gap is recorded as X-250; the coordinator ruled on 2026-10-29 at 10:27 to keep parity and not block.

- **D-281 X-251 closed: a Go server and a Go worker beside the Rust master (court verdict, 2026-10-29).** `claude/project-thread-lryfg0` at `3933512`, one commit on `9ed33cd` (the D-280 verdict), is fast-forwarded into the integration branch. Four files change: `conformance/tests/mixed_master.rs` (a new test, 230 lines), `conformance/Cargo.toml` and `Cargo.lock` (the test's dependencies), and `porting/notes/WP-4-4.md`.

  The new test, `rust_master_serves_go_nodes_recommendations`, builds a Go server and a Go worker with `go build`, and runs them once under the Rust master and once under the Go master, on the same seeded data. It asserts that both nodes register as `Server` and `Worker` under both masters; that the masters give the same Recommend config and the Go ranker's recommenders; that each user's Go-served list is non-empty and shares an item with the collaborative rows the Go worker wrote; and that the lists served under the Rust master equal those served under the Go master. The diff adds no `#[ignore]`, `#[allow]` or skip.

  Gate on `3933512`, every backend URI set: every stage exits 0 except REDIS_TLS_UNSET (101, expected). TEST: 1277 passed, 0 failed, 55 ignored. The new test passes in that run, as do `master_rest_matches_go`, `null_external_matches_go` and the live `mixed_mongo_deep_labels_match_go_server`.

  WP-4-4 is complete. X-250 (OIDC state and nonce) stays a logged upstream gap for the coordinator, not a blocker.

- **D-282 O-87 accepted and closed: X-246 closed with no deviation, X-247 approved as a test-oracle tie grouping, X-248 and X-249 resolved (court verdict, 2026-10-29).** The O-87 head `e9a33b3` (`claude/o-87-flakes`, base `3a85214`) is merged into the integration branch as `8306391`, after `75c10ab`. Two rulings of the coordinator (13:14) are recorded here: X-246's closure is approved, so the single-query change is reverted (`06dd0d8`) and Go's paged `UpdateScores` is the oracle; and the X-247 f64-bits tie grouping is approved as test-oracle only.

  Gate on `8306391`, every backend URI set: every stage exits 0 except REDIS_TLS_UNSET (101, expected). TEST: 1279 passed, 0 failed, 55 ignored. The generic `test_mysql` and `test_postgres` suites pass, and so does the live MIXEDMONGO test.

  Checks by the court beyond the gate:
  - X-248, the 1,000-row user and feedback statements. Throwaway probes (not committed; the tree was restored) inserted 12,000 items, 12,000 users and 12,000 feedback rows, one call each, on MySQL and on PostgreSQL. All succeed. Exact row counts by SQL: items 24,000, users 24,000, feedback 12,000 on both. The 24,000 includes the users and items that `BatchInsertFeedback` creates.
  - One probe run reported 0 users on PostgreSQL. The cause is `count_table`: on PostgreSQL it returns `reltuples` from `pg_class`, an estimate that stays stale until `ANALYZE`. Go does the same (`storage/data/sql.go:1522`, `1548`, `1574`). The rows were there, and the SQL counts are exact. This is Go parity, not a defect. MySQL's `SHOW TABLE STATUS` count is also an estimate; the SQL counts above are exact.
  - X-247. In `conformance/tests/recommend_oracle.rs` the comparison checks every score, the order between groups, the item subset and count within each tie group, and the digest, which is the cache-key digest, not the list order. A group that the limit cuts off is checked against Go's wide list. Where there is no wide list, its items are reported as unverified in the test output, not passed silently.
  - `629646e`. The `scan_scores` timer race is replaced by a cancel after the first score. The assertion (fewer than nine scans) is unchanged. The race is removed, not hidden.
  - The regression test removed with the single-query change (`update_scores_with_tied_scores_on_fresh_indexes`) belonged to the reverted code. Go's paged loop is now the oracle, and it passed 0 of 1,000 lost updates on the court's probe.

  Coordinator ruling (13:33): X-252 is an upstream quirk, since Go and the port lose hashes at the same rate under load and neither does idle. It does not block the Phase 4 exit, and it is a Phase 5 conformance watch item.

  X-252 is logged: Go lost 66 hashes of 1,000 trials once, under a busy server, and the port's paged loop lost 68 once, earlier. Neither reproduces on an idle server (0 of 1,000 each). The cause is not found. On the evidence it is not a port defect, and the port keeps Go's loop.

  Evidence: `/mnt/project-files/o87-evidence/court-batch-probe/`.

- **D-283 WP-4-5 binaries accepted, with X-254 open (court verdict, 2026-10-30).** `claude/wp-4-5-rebased` at `4ed8d65`, rebased onto `bf9f375`, is merged into the integration branch as `f93c613`, on top of the X-253 record. The binaries are ported: `gorse-master`, `gorse-server`, `gorse-worker` and `gorse-in-one`, with `cmd/version`. `cmd/goat` is not ported (D-238), and `gorse-bench` is WP-4-8.

  Gate on `f93c613`, every backend URI set (MIXEDMONGO with `?authSource=admin`): every stage exits 0 except REDIS_TLS_UNSET, which exits 101 as required. TEST: 1299 passed, 0 failed, 56 ignored. The WP-4-5 tests pass: the five `mixed_cmd` scenarios, which run each Rust binary beside the Go nodes (the master, server, worker and in-one binaries, and the help check), `rust_binaries_help_matches_the_help_files`, and the `gorse-cmd` contract and binary tests. The one ignored playground test is `#[ignore = "O-1: dataset from cdn.gorse.io"]`, and its dataset is missing.

  Reviewed by reading: the diff adds no skip. The ignore is the O-1 playground test, which is valid. No assertion is removed.

  Deviations, logged in `porting/notes/WP-4-5.md` (1 to 11): Go's `--http-host` and `--http-port` help texts are swapped, and the port keeps Go's text; `--playground` takes no separate value; `-v` and `--help` create the default cache directory; usage errors exit 1; Go's `gorse-in-one` starts no server or worker, and the port says so. The court accepts them as faithful to Go.

  Cross-crate: `Master::restore` in `crates/gorse-master/src/rest/dump.rs` (9b0beee, approved by the coordinator), and the helpers in `conformance/src/cluster.rs`.

  Open: X-254 (the master HTTP routes are not served, and CORS is `*`). Not blocking this node's acceptance, but it blocks the Phase 4 exit. Also noted, not blocking: the stale doc comment at `crates/gorse-worker/src/worker.rs:74` (WP-4-1, report only).

- **D-284 X-254 accepted for the master: CORS, Swagger UI and pprof served (court verdict, 2026-10-30).** `claude/x-254-master-router` at `48bf5a1` (one commit on `8a773a7`) is merged into the integration branch as `fa7b2ab`. The master's go-restful CORS filter (the configured `http_cors_domains` and methods), `/apidocs.json`, `/apidocs/` (the vendored swgui v1.8.5 bundle), and the `/debug/pprof/` routes are now served.

  Gate on `fa7b2ab`, every backend URI set (MIXEDMONGO with `?authSource=admin`): every stage exits 0 except REDIS_TLS_UNSET, which exits 101 as required. TEST: 1314 passed, 0 failed, 56 ignored. The new `master_http_surface` tests pass against the Go master: `restricted_cors_matches_go`, `default_cors_matches_go`, `apidocs_matches_go` and `pprof_routes_match_go`. `mixed_master_rest` (WP-4-4) still passes.

  Reviewed by reading: no test is skipped or removed. The `Skip` names in the diff are the `BodyCheck::Skip` enum variant for the pprof bodies, which X-254a documents.

  Deviations, logged in `PORTING_NOTES.md` as X-254a to X-254g: the pprof profile bodies are empty where Go's are sampled (X-254a); the Swagger files are decompressed (X-254b); the OpenAPI `required` arrays are sorted (X-254c, Q-8); the method table is generated, so a new Go route needs a new row (X-254d); the origin merge follows the content type (X-254e); and go-restful's 404 and 405 texts on WebService paths (X-254f).

  Still open: `gorse-server` does not serve `/apidocs`, pprof, or the CORS filter. X-214 and X-215 stay open for it, and they block the Phase 4 exit. This branch changes only the master. The CORS wildcard that X-254 removes from the master also remains in `gorse-server` until X-214 closes.

- **D-285 X-214 and X-215 merged, not accepted: the gate fails in `gorse-server` (court verdict, 2026-10-30).** `claude/x-214-server-http` at `9b7a80f` (on `287872f` and `daa77e6`) is merged into the integration branch as `2bccfae`. The merge stays, and it is not accepted.

  Gate on `2bccfae`, every backend URI set (MIXEDMONGO with `?authSource=admin`): REDIS_TLS_UNSET exits 101, as required. Two stages fail:

  - DOC exits 101. Rustdoc with `-D warnings` reports five `public documentation for cors links to private item` errors in `crates/gorse-server/src/rest/cors.rs`: `Cors::is_web_service`, `merge_origin`, `restful_errors`, `Source::Fixed` and `GoList`. The file is new in `287872f`. The gate on `8306391`, before X-214, had DOC at 0.
  - TEST exits 101. `server_http_surface::default_cors_matches_go` fails. The Rust node answers the default CORS preflights differently from Go: the allow-origin header echoes `https://allowed.example` where Go sends none; the allow-methods header is `GET,POST,PUT,DELETE,PATCH` where Go sends `PATCH,GET,DELETE` (a parameter path), `GET,POST` (a refused origin, and a preflight without requested headers) or nothing; the allow-headers header is `Content-Type, x-api-key` where Go sends nothing.

  TEST: 1329 passed, 1 failed, 56 ignored. Elsewhere on this head: `master_http_surface` 5 of 5; `server_http_surface` 4 of 5 (`apidocs_matches_go`, `restricted_cors_matches_go`, `server_only_routes_match_go` and `pprof_routes_match_go` pass); `mixed_master_rest` 3; `mixed_server` 2; `metrics_matches_go` passes; `mixed_mongo_deep_labels_match_go_server` ran with `MONGO_URI` and passes. The two `gorse-storage --lib` TLS failures are in the REDIS_TLS_UNSET stage, where exit 101 is required.

  The thread reported `server_http_surface` 5 of 5 on its own head. `daa77e6..2bccfae` touches only the master and its test, so the server CORS code that fails here is the thread's own. The court ran `default_cors_matches_go` once on the merged head. A second run could not be made, because the disk filled while the Go oracle rebuilt, so this record rests on one run. The X-214 thread must rerun the test on `9b7a80f` with the Go oracle and show the output.

  Not accepted until the X-214 thread pushes a head on which `cargo doc` with `-D warnings` passes, and `default_cors_matches_go` passes against the Go node. The CORS table is the Go oracle. No test may be skipped, removed or weakened. The court then reruns the full gate on the merged head, with every backend URI set.

  X-214 and X-215 stay open. The master `/metrics` content type from `9b7a80f` is the coordinator-approved fix, and `metrics_matches_go` passes on this head.

- **D-286 WP-3-8 metric parity accepted on the merged head; the red stages are X-214's two known failures (court verdict, 2026-10-30).** `claude/wp-3-8-metrics` at `ba1a4fe` (fix `aaafadf`, which shares the built-in dictionaries as Go does, C-36; un-ignores the O-1 tests whose data is in the project folder) is merged into the integration branch as `a598d71`, on `60887bd`. Metric parity is the last Phase 3 node, so Phase 3 is accepted on this verdict.

  Gate on `a598d71`, every backend URI set (MIXEDMONGO with `?authSource=admin`). Logs are in `/mnt/project-files/reports/wp-3-8/`.

  - Exit 0: FMT, CLIPPY, CONFIGGOLDEN, YAMLGOLDEN, VET, CLOUDCLIPPY, MONGOCLIPPY, VECTOR_ACCEPT, CROSSREAD, PROXY_CONFORMANCE, PROXY_STORE, and MIXEDMONGO (`mixed_mongo_deep_labels_match_go_server` ok, with `MONGO_URI`).
  - METRICS, the release run of the full metrics suite (`cargo test --release -p gorse-conformance --test metrics`): 15 passed, 0 failed, finished in 1899.59 s. The debug run of the same suite was too slow for the gate, so the court ran it as its own release stage, as the coordinator ruled.
  - REDIS_TLS_UNSET: exit 101, as required. The two failures are `tls_skip_verify_connects_to_a_self_signed_server` and `tls_verifies_the_server_by_default`.
  - Red, attributed to X-214 (D-285), unchanged:
    - DOC exits 101 with the same five rustdoc errors in `crates/gorse-server/src/rest/cors.rs` (`Cors::is_web_service`, `merge_origin`, `restful_errors`, `Source::Fixed`, `GoList`).
    - TEST, the conformance targets only: `server_http_surface::default_cors_matches_go` fails in one run. The rest of TEST is green: the workspace without `gorse-conformance` (1274 passed, 0 failed, 45 ignored), and the conformance targets (64 passed, 1 failed, 2 ignored). The gate script's own TEST line also exited 101 on a wrong target name, `data`, which is a directory and not a test target. The conformance targets were rerun with the same environment (`gate-testb2.log`), and those counts are the ones above.

  `default_cors_matches_go` is flaky on this head. It passed in one court run (`gate-testb.log`, which had no backend URIs and did not need them), failed in the next (`gate-testb2.log`), and failed in the D-285 run on `2bccfae`. That fits the start-order race the X-214 owner describes (Go's CORS copy against its first meta sync: `server/server.go:130-133` and `server/rest.go:129-146`). The harness-order change in `fbfd5ab` is not merged or ruled on yet, and X-214 stays open.

  The metric evidence is the release run above: the 15 metrics tests pass against Go at the tolerances in D-195 and the X-253 band, which include the NumCPU BPR runs and the one-job replay. The court did not re-count the NumCPU runs by hand. Q-269 is Go's own parallel-BPR spread and X-253 is the Rust deviation; both are in `porting/notes/WP-3-8.md` and `PORTING_NOTES.md`. C-36 is the shared-dictionary fix, with count evidence in the commit.

  Still open for the Phase 4 exit: X-214 and X-215 (the `fbfd5ab` review, then the gate on the integration head), WP-4-6 (`4ccd665`), WP-4-7 (blocked on `ml-100k.bin.gz`, O-1), WP-4-8 (`dc2e39b`), X-239, Q-265, and O-1.

- **D-287 X-214 and X-215 accepted on the merged head, with the CORS harness order pinned (court verdict, 2026-10-30).** `claude/x-214-server-http` at `fbfd5ab` (the D-285 fixes, on `9b7a80f`) is merged into the integration branch as `8c196fd`, on `6f2036a`. The coordinator approved the harness order on 2026-10-30 (07:33Z) on three conditions, and all three are met.

  Gate on `8c196fd`, every backend URI set (MIXEDMONGO with `?authSource=admin`). Log: `/mnt/project-files/reports/x-214/court-gate-8c196fd.log`. Every stage exits 0 except REDIS_TLS_UNSET, which exits 101 as required. DOC exits 0, so the five `cors.rs` links are fixed. TEST: 1339 passed, 0 failed, 47 ignored, including `default_cors_matches_go` in `server_http_surface` and `master_http_surface`. METRICS (release): 15 passed, 0 failed. `mixed_mongo_deep_labels_match_go_server` passes with `MONGO_URI`.

  Condition 1, both orders under load (`order-experiment.log`). The court put a temporary master-first variant into `go_answers`, ran `default_cors_matches_go` against the Go node, and restored the file from HEAD.
  - Master first (the sync-first order): under load, 9 of 10 passed and 1 failed (run 6). Idle, 10 of 10 passed. The failure was captured on a second pass (`court-sync-first-failure.log`). The Go node answered with an empty allow-origin list, no allow-methods on allowed-origin preflights, and no allow-headers. Those are the answers in D-285.
  - Start first (the pinned order): under load, 10 of 10 passed. Idle, 10 of 10 passed.
  - The load was four busy loops on four cores. The X-214 owner reported 3 of 4 failures under its own load. The court's rate is lower, and the ruling does not depend on the rate.

  Condition 2, the Go quirk, is logged as X-255 in `PORTING_NOTES.md`.

  Condition 3, `default_cors_matches_go` in 10 consecutive runs on the merged head with DOC green: 20 consecutive passes in the start-first runs (10 under load, then 10 idle), on the restored file at `8c196fd`. The gate's TEST stage passed as well.

  The fix pins Go's causal copy-first order, which Go takes in production. Rust's copy-first model, the test cases and the assertions are unchanged. The `free_port` change (10000 to 31999, reserved per process) removes an ephemeral-port flake.

  X-214 and X-215 are closed for the server. The master `/metrics` content type (X-214b) is in this head. Still open for the Phase 4 exit: WP-4-6 (`4ccd665`), WP-4-7 (blocked on O-1), WP-4-8 (`dc2e39b`), X-239, Q-265 and O-1.

- **Datasets.** The network policy of this environment blocks
  `https://cdn.gorse.io`, where the Go model tests and the SDK test setup download
  their datasets (`PORTING_NOTES.md` O-1). Phase 3 metric parity needs either
  that host allowed or the dataset files placed in the repository's environment.

- **D-279 Cross-crate deviation: the external pull keeps a `null` list, and the tasks answer is pinned (WP-4-4 follow-up, O-76; renumbered at the court merge).** WP-3-7's `gorse-logics` `External::pull` returned an empty vector for a script whose list is `null`. Go unmarshals `null` into a nil slice, and the master's external route answers that as `null`, so the port answered `[]`. `pull` now returns `Option<Vec<String>>`, with `None` for `null`. The recommender takes the empty list (its path keeps no nil, D-235), and the master's external route answers `null` on `None`. The change touches WP-3-7's crate, approved by the coordinator. It is pinned by the `gorse-logics` unit tests `null_result_has_no_items` and `empty_list_result_has_no_items`, and by conformance `mixed_master_rest::null_external_matches_go`, which runs the Go master. The empty task list is pinned by the Go test `TestEmptyListMarshalsAsNull` (`common/monitor/progress_test.go`), which conformance `monitor_empty_list` runs, and by the Rust test `get_tasks_is_null_without_tasks`.
