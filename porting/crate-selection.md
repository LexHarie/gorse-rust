# Crate selection

Versions are the latest stable release on crates.io on 2026-10-17 (queried from
`https://crates.io/api/v1/crates/<name>`), as Ritchie asked. Every crate listed is
under a permissive licence compatible with Apache-2.0. Toolchain: Rust 1.99.1
stable, edition 2024 (`rust-toolchain.toml`).

Status: decided by the coordinator under Ritchie's instruction to build
autonomously (`PORTING.md` decision D-3). A crate is added to a crate's
`Cargo.toml` only by the work package that first needs it, so the workspace never
carries unused dependencies.

## Library crates

| Go dependency | Rust crate | Version | Licence | Why | Alternatives considered |
|---|---|---|---|---|---|
| grpc-go, protobuf-go | `tonic`, `prost`, `tonic-prost` | 0.14.6, 0.14.4, 0.14.6 | MIT, Apache-2.0 | The standard Rust gRPC stack; generated code is wire-compatible with protobuf-go | `grpcio` (C++ core, heavier build) |
| `protoc` at build time | `tonic-prost-build` + `protox` | 0.14.6, 0.10.0 | MIT, MIT/Apache | `protox` compiles the `.proto` files in pure Rust, so builds need no system `protoc` (none is installed here) | system `protoc` via `prost-build` (extra build dependency) |
| go-restful | `axum` + `tower-http` | 0.8.9, 0.7.1 | MIT | Most used Rust HTTP server on hyper/tokio; filters map to tower layers | `actix-web` (own runtime), `warp` (less maintained) |
| go-restful-openapi | none: the frozen `contracts/rest/*.openapi.json` is embedded and served | — | — | The Go nodes serve **OpenAPI 2.0**; `utoipa` only emits OpenAPI 3.x, so it cannot reproduce `/apidocs.json` (plan correction C-5) | `utoipa` 6.0.0 (wrong spec version) |
| swaggest/swgui | Swagger UI assets taken from the pinned Go module | — | Apache-2.0 | Same UI files the Go node serves; `utoipa-swagger-ui` downloads assets from GitHub at build time | `utoipa-swagger-ui` 10.0.1 |
| gorm + MySQL/Postgres drivers | `sqlx` (`mysql`, `postgres`) | 0.9.0 | MIT/Apache | Async, explicit SQL, the plan's choice; we write the SQL gorm generates by hand | `diesel` (ORM, sync), `sea-orm` (ORM on sqlx) |
| gorm sqlite (gorse-io fork on modernc) | `rusqlite` (`bundled`) on `spawn_blocking` | 0.40.2 | MIT | Gorse registers a custom `json_contains` SQL function per connection; `rusqlite` exposes `create_scalar_function` safely, `sqlx-sqlite` only through a raw handle and `unsafe`. `sqlx`'s `sqlite` feature stays off so only one `libsqlite3-sys` is linked | `sqlx` `sqlite` |
| mongo-driver | `mongodb` | 3.9.1 | Apache-2.0 | Official driver | — |
| go-redis (standalone, cluster, RediSearch, RedisTimeSeries) | `redis` (`tokio-comp`, `cluster-async`) | 1.7.1 | BSD-3-Clause | Most used client; cluster support; `FT.*`/`TS.*` sent as raw commands like go-redis does | `fred` 10.1.0 (smaller user base) |
| qdrant go-client | `qdrant-client` | 1.19.0 | Apache-2.0 | Official client, gRPC like the Go one | hand-written gRPC |
| milvus client v2 | hand-written adapter over the Milvus REST v2 API with `reqwest` | — | — | `milvus-sdk-rust` 3.0.2 declares a non-standard licence, which the plan forbids | `milvus-sdk-rust` |
| weaviate-go-client | hand-written REST + GraphQL adapter with `reqwest` | — | — | `weaviate-community` 0.2.2 is unmaintained since 2024-02 | `weaviate-community` |
| minio-go, GCS, Azure SDKs | `object_store` (`aws`, `gcp`, `azure`) | 0.14.2 | MIT/Apache | One API over the three clouds, maintained by Apache Arrow | the three official SDKs |
| blob stores (streams, byte buffers, URL and key escaping) | `tokio-stream`, `futures-util`, `bytes`, `url`, `percent-encoding` | 0.1.19, 0.3.34, 1.12.1, 2.5.8, 2.3.2 | MIT/Apache | Used by the cloud blob stores (WP-2-10). Latest stable verified on crates.io, 2026-10-28 | — |
| data store streams (`BoxStream` for the Go channels) | `futures-core`, `futures-util` | 0.3.34, 0.3.34 | MIT/Apache-2.0 | Go's channel pairs become one stream per call (D-146); `futures-util` is the stream combinator crate. Latest stable verified on crates.io, 2026-10-28 | — |
| net/http client | `reqwest` (`rustls`) | 0.13.5 | MIT/Apache | Standard | `hyper` directly |
| crypto/tls, crypto/x509, grpc `advancedtls` (`util.NewServerCreds`, `NewClientCreds`) | `rustls` (`aws_lc_rs`, `tls12`; a patched copy in `third_party/rustls`), `rustls-webpki`, `x509-parser`; crypto by `aws-lc-rs` through `rustls` | 0.23.46, 0.103.15, 0.18.1, 1.18.2 | Apache-2.0/ISC/MIT, ISC, MIT/Apache, ISC/Apache | `rustls` is what `tonic` and `reqwest` use. `webpki` builds the chain, and a path callback adds the checks Go's `x509.Verify` makes on the CA certificate, which a `webpki` trust anchor drops (validity, extended key usage, path length, key usage); `x509-parser` reads those fields. `aws-lc-rs` because Gorse's own test certificates are P-521, which `ring` cannot verify. The patch makes a server ignore the `host:port` server name Go clients send instead of failing the handshake (Ritchie's decision, 2026-10-18; `third_party/rustls/GORSE.md`, WP-1-4 notes) | the `ring` provider (no P-521), `native-tls`/`openssl` (system libraries, different verification) |
| async runtime | `tokio` | 1.54.1 | MIT | Required by tonic/axum/sqlx; the runtime under the async storage traits (D-122) | — |
| async methods on the storage traits used as `dyn` | `async-trait` | 0.1.92 | MIT/Apache-2.0 | Go interfaces are used as values (`Open` returns the store), so the trait must be dyn-compatible, and native `async fn` in traits is not (D-122) | native `async fn` in traits (not dyn-compatible) |
| viper + mapstructure + validator | `toml` + `serde` + hand-written viper layering and validation | 1.1.8, 1.0.229 | MIT/Apache | Viper's rules (case-insensitive keys, defaults, env bindings in `config.keys.md`, Go duration strings, weak typing of env values) must be reproduced exactly; a generic layered-config crate would add its own rules | `figment` 0.10.19, `config` 0.15.27 |
| invopop/jsonschema | none: the frozen `contracts/config/config.schema.json` is served | — | — | The schema is a contract, so it is embedded rather than regenerated | `schemars` 1.2.3 (different output) |
| cobra / pflag | `clap` (`derive`) | 4.6.7 | MIT/Apache | Standard; reproduces pflag syntax including `NoOptDefVal` via `require_equals` + `default_missing_value` | `argh`, `bpaf` |
| zap + lumberjack | `tracing`, `tracing-subscriber`, `tracing-appender`, `file-rotate` | 0.1.44, 0.3.23, 0.2.5, 0.8.0 | MIT | Standard logging; `file-rotate` gives size-based rotation with max backups like lumberjack | `log` + `flexi_logger` |
| otel + otlp/zipkin exporters | `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-zipkin`, `tracing-opentelemetry` | 0.33.1, 0.33.1, 0.33.1, 0.33.0, 0.34.0 | Apache-2.0 (`tracing-opentelemetry` MIT) | Official SDK; the three Go exporters (`zipkin`, `otlp`, `otlphttp`) all exist | — |
| prometheus client_golang | `prometheus` (`process`) | 0.14.0 | Apache-2.0 | Same exposition format and metric model | `metrics` + exporter |
| fsnotify | `notify` | 8.2.0 | CC0-1.0 | Standard file watcher | polling |
| statik (dashboard bundle) | `zip` + `rust-embed` | 9.1.3, 8.13.0 | MIT | The dashboard is a zip inside the Go module `gorse-io/dashboard`; the build extracts that exact zip and embeds it | committing the bundle (generated artefact, forbidden) |
| expr-lang/expr | hand port of expr-lang v1.17.8 in `gorse-expr`: lexer and parser (WP-1-19), checker, compiler and VM (WP-3-6) | — | — | Gorse uses expr-lang to validate the `item_expr` config fields (`parser.Parse`) and to run the recommend filters and scores in `logics`; `common/expression` is unrelated (`PORTING.md` C-12). Parity is tested against verdicts and results from Go (risk 3). `expr-lang` 2.1.0 is a separate re-implementation whose behaviour is unverified; WP-3-6 evaluates it first and keeps it only if it passes the corpus | `expr-lang` 2.1.0, `evalexpr` (AGPL, forbidden), `rhai` (different language) |
| (Go's growable goroutine stacks, which expr-lang's recursive parser and tree walks rely on) | `stacker` | 0.1.25 | MIT OR Apache-2.0 | Grows the stack on demand, so `gorse-expr` parses, prints, walks and compares trees of any depth the parser accepts, as Go does (a config may turn the node budget off). `gorse-config` uses it the same way for go-toml's recursive parser, key tracker and decoder, and for the conversion of decoded YAML to `Value` (WP-1-15), so a TOML or YAML config nests as deep as Go accepts (Go stops at its 1 GB stack limit; the port has none). Maintained by the Rust project (used by rustc) | explicit-stack rewrites of each recursive function (the parser's recursion is its structure; rewriting it obscures the line-by-line port), a big fixed thread stack (still a limit) |
| gonja | `minijinja` | 3.0.1 | Apache-2.0 | Jinja2-compatible, mature; parity corpus from Go tests | `tera` (not Jinja-compatible) |
| modernc quickjs | `rquickjs` | 0.14.0 | MIT | Same QuickJS engine, so script semantics match | `boa` (different engine) |
| go-openai | `reqwest` + serde structs mirroring the go-openai types Gorse uses | — | — | The master decodes into go-openai's typed request and re-encodes it, dropping unknown fields; mirroring those structs reproduces that exactly | `async-openai` 0.42.2 (different field set) |
| go-oidc + oauth2 | `openidconnect` | 4.0.1 | MIT | Standard OIDC client with ID-token verification | hand-written |
| gorilla/securecookie | `cookie` (`private`) | 0.18.2 | MIT/Apache | Cookies are only read by the process that wrote them (random keys per start), so any authenticated encryption works | port of securecookie |
| jellydator/ttlcache | `moka` | 0.12.16 | MIT/Apache | TTL cache | `mini-moka` |
| juju/ratelimit | hand port of v1.0.2 (`ratelimit.go`, `reader.go`) in its own crate, `gorse-ratelimit`, under juju's licence: `(LGPL-3.0-only WITH LGPL-3.0-linking-exception) AND BSD-3-Clause` (the BSD part is a few lines of Go's `strconv` for a panic message) | — | — | Gorse builds its OpenAI limiters on juju's bucket; in add21b5 they are only created (`master/master.go:137-138`), so what must match today is the start-up panic for a limit of 1 to 59 per minute and the bucket arithmetic, which the port reproduces exactly (checked against Go). A separate LGPL crate keeps the rest of the workspace Apache-2.0; the static-linking exception lets the binaries link it (Ritchie's decision, 2026-10-19) | `governor` 0.10.4 (GCRA, different burst behaviour), a clean-room Apache port |
| Go `context` (cancellation) | hand port in `gorse-common::gocontext` of `Background`, `WithCancel` and the `context canceled` error | — | — | Gorse's parallel schedulers take a `context.Context` and stop when it is cancelled; callers block on it from plain threads, which `tokio-util`'s `CancellationToken` (async only) does not offer | `tokio-util` 0.7.20 `CancellationToken` |
| lafikl/consistent + blake2b-simd | hand port + `blake2` | 0.11.0 | MIT/Apache | Worker user partitioning must match Go bit for bit (`contracts/grpc.md`) | `hashring` crates (different hashing) |
| araddon/dateparse | hand port of `ParseAny` and the helpers Gorse calls | — | — | Risk 7: every loose format must parse identically; `dateparser` 0.3.1 accepts a different set | `dateparser` |
| encoding/gob | hand-written subset codec in `gorse-common` | — | — | Only the types in `contracts/model-blobs.md` are needed; `gob` 0.1.0 is a 2018 prototype | `gob` 0.1.0 |
| chewxy/math32 | hand port in `gorse-common::math32` of the functions Gorse calls, with amd64's assembly `Exp`, `Log` and `Sqrt` reproduced (WP-1-11) | — | — | Model, ANN and optimizer numbers depend on math32's exact bits, which differ from correctly rounded results (its amd64 `Exp` has a coefficient bug); checked against Go on all 2^32 inputs | `f32::exp`/`f32::ln` (platform libm) and the `libm` crate: near correctly rounded, so they cannot give math32's bits |
| math/rand | hand port of Go's `math/rand` source (the only RNG package Gorse imports) | — | — | Risk 2: tests depend on exact sequences | `rand` 0.10.3 (different streams) |
| goptuna TPE | hand port of the sampler Gorse calls | — | — | Plan says manual port | — |
| gomlx / go-xla (`fm_xla.go`) | not ported; `fm.go` + `common/nn` ported by hand | — | — | Default Go builds use `fm.go`; see decision D-8 | `candle` 0.11.0, `burn` 0.22.0 |
| common/floats SIMD | C sources in `common/floats/src` built with `cc`; runtime detection with `std::arch` | `cc` 1.8.0 | MIT/Apache | The plan's approach; same kernels as Go | `std::simd` (unstable) |
| x/exp/float16 | `half` | 2.7.1 | MIT/Apache | FP16 vectors on the wire | — |
| google/uuid | `uuid` (`v3`, `v4`) | 1.28.0 | MIT/Apache | — | — |
| crypto/md5 | `md-5` | 0.11.0 | MIT/Apache | — | — |
| bits-and-blooms/bitset | `fixedbitset` | 0.5.7 | MIT/Apache | Small, dependency-free | `bitvec` 1.1.1 |
| compress/gzip | `flate2` | 1.1.10 | MIT/Apache | — | — |
| JSON | `serde`, `serde_json` | 1.0.229, 1.0.154 | MIT/Apache | — | — |
| encoding/csv (`datautil.LoadIris`) | `csv` | 1.4.0 | Unlicense/MIT | Read with no header row, as Go's `csv.NewReader` reads it; blank lines are skipped and the first record fixes the field count | — |
| archive/zip (`datautil` unzip) | `zip` | 9.1.3 | MIT | Entries are checked against the destination before they are written (ZipSlip), as Go's `unzip` does; file modes are not restored | — |
| encoding/json where Go's decoding is observable (`json.Unmarshal` into `any`, `[]any`, `map[string]any`) | hand port `gorse_common::gojson`, objects in `indexmap` | 2.14.2 | MIT/Apache | Go 1.27 runs v1 `Unmarshal` on the v2 decoder; its error texts, duplicate names, invalid UTF-8, nesting limit and float overflow reach users through `json_contains` and config errors, and `serde_json` differs on each. `indexmap` keeps object names in document order (WP-1-14, WP-1-15) | `serde_json` with `preserve_order` (other errors and rules) |
| crypto/sha1 (gorm's `formatName`) | `sha1` | 0.11.0 | MIT/Apache | gorm cuts long index and constraint names and appends SHA-1 hex digits (WP-1-14) | — |
| regexp (RE2) | `regex` | 1.13.1 | MIT/Apache | Same leftmost-first semantics and anchoring as RE2 for the patterns Gorse uses, so `common/expression` keeps the Go pattern verbatim; checked against a Go corpus (WP-1-7) | a hand-written matcher (equivalent here, but no longer the Go pattern) |
| time | none: hand port of Go's `time` (`gorse_common::gotime`) | n/a | n/a | Go's exact layouts, parse errors, `Time.String()` with the monotonic suffix, `ParseDuration` and the TZif reader; `chrono` would contribute nothing (`PORTING.md` C-13) | `time` |
| errors | `thiserror` | 2.0.21 | MIT/Apache | Typed errors so callers can match kinds like Go's `errors.Is` | — |
| base64 | `base64` | 0.23.1 | MIT/Apache | Redis category encoding | — |
| schollz/progressbar | `indicatif` | 0.18.6 | MIT | Playground import progress only | — |
| samber/lo, golang-set, copier, sortutil, strutil | std | — | — | Small helpers; written inline | — |

Test-only: `tempfile` 3.27.0; `hex` 0.4.3 (MIT/Apache) for the golden files;
`sha2` 0.11.1 (MIT/Apache) for the hashes in `floats_golden.json`. Storage tests use the real backends from
`storage/docker-compose.yml`, never mocks.

## Not adopted from the plan's proposal

| Proposed | Reason |
|---|---|
| `utoipa` | Emits OpenAPI 3; the contract is OpenAPI 2.0 (C-5) |
| `candle` or `burn` | The default FM is the pure-Go `fm.go` on `common/nn`; porting that keeps blob compatibility and numerics (C-4, D-8) |
| `figment` or `config` | Viper's merge rules must be reproduced exactly; a generic crate adds different rules |
| `fred` | `redis` covers standalone, cluster and raw module commands with a larger user base |
| `async-openai` | Would change which request fields survive the master's proxy |
