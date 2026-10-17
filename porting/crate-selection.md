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
| net/http client | `reqwest` (`rustls`) | 0.13.5 | MIT/Apache | Standard | `hyper` directly |
| async runtime | `tokio` | 1.53.3 | MIT | Required by tonic/axum/sqlx | — |
| viper + mapstructure + validator | `toml` + `serde` + hand-written viper layering and validation | 1.1.8, 1.0.229 | MIT/Apache | Viper's rules (case-insensitive keys, defaults, env bindings in `config.keys.md`, Go duration strings, weak typing of env values) must be reproduced exactly; a generic layered-config crate would add its own rules | `figment` 0.10.19, `config` 0.15.27 |
| invopop/jsonschema | none: the frozen `contracts/config/config.schema.json` is served | — | — | The schema is a contract, so it is embedded rather than regenerated | `schemars` 1.2.3 (different output) |
| cobra / pflag | `clap` (`derive`) | 4.6.7 | MIT/Apache | Standard; reproduces pflag syntax including `NoOptDefVal` via `require_equals` + `default_missing_value` | `argh`, `bpaf` |
| zap + lumberjack | `tracing`, `tracing-subscriber`, `tracing-appender`, `file-rotate` | 0.1.44, 0.3.23, 0.2.5, 0.8.0 | MIT | Standard logging; `file-rotate` gives size-based rotation with max backups like lumberjack | `log` + `flexi_logger` |
| otel + otlp/zipkin exporters | `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, `opentelemetry-zipkin`, `tracing-opentelemetry` | 0.33.1, 0.33.1, 0.33.1, 0.33.0, 0.34.0 | Apache-2.0 (`tracing-opentelemetry` MIT) | Official SDK; the three Go exporters (`zipkin`, `otlp`, `otlphttp`) all exist | — |
| prometheus client_golang | `prometheus` (`process`) | 0.14.0 | Apache-2.0 | Same exposition format and metric model | `metrics` + exporter |
| fsnotify | `notify` | 8.2.0 | CC0-1.0 | Standard file watcher | polling |
| statik (dashboard bundle) | `zip` + `rust-embed` | 9.0.4, 8.13.0 | MIT | The dashboard is a zip inside the Go module `gorse-io/dashboard`; the build extracts that exact zip and embeds it | committing the bundle (generated artefact, forbidden) |
| expr-lang/expr | hand-written evaluator for the expression subset Gorse accepts | — | — | Gorse uses expr-lang only through `common/expression` and the recommend filters; parity is tested against a corpus from the Go tests (risk 3). `expr-lang` 2.1.0 is a separate re-implementation whose behaviour on that corpus is unverified; WP-3-6 evaluates it first and keeps it only if it passes the corpus | `expr-lang` 2.1.0, `evalexpr` (AGPL, forbidden), `rhai` (different language) |
| gonja | `minijinja` | 3.0.1 | Apache-2.0 | Jinja2-compatible, mature; parity corpus from Go tests | `tera` (not Jinja-compatible) |
| modernc quickjs | `rquickjs` | 0.14.0 | MIT | Same QuickJS engine, so script semantics match | `boa` (different engine) |
| go-openai | `reqwest` + serde structs mirroring the go-openai types Gorse uses | — | — | The master decodes into go-openai's typed request and re-encodes it, dropping unknown fields; mirroring those structs reproduces that exactly | `async-openai` 0.42.2 (different field set) |
| go-oidc + oauth2 | `openidconnect` | 4.0.1 | MIT | Standard OIDC client with ID-token verification | hand-written |
| gorilla/securecookie | `cookie` (`private`) | 0.18.2 | MIT/Apache | Cookies are only read by the process that wrote them (random keys per start), so any authenticated encryption works | port of securecookie |
| jellydator/ttlcache | `moka` | 0.12.16 | MIT/Apache | TTL cache | `mini-moka` |
| juju/ratelimit | hand port (token bucket, ~100 lines) | — | — | Exact refill semantics matter for the 429 behaviour | `governor` 0.10.4 (GCRA, different burst behaviour) |
| lafikl/consistent + blake2b-simd | hand port + `blake2` | 0.11.0 | MIT/Apache | Worker user partitioning must match Go bit for bit (`contracts/grpc.md`) | `hashring` crates (different hashing) |
| araddon/dateparse | hand port of `ParseAny` and the helpers Gorse calls | — | — | Risk 7: every loose format must parse identically; `dateparser` 0.3.1 accepts a different set | `dateparser` |
| encoding/gob | hand-written subset codec in `gorse-common` | — | — | Only the types in `contracts/model-blobs.md` are needed; `gob` 0.1.0 is a 2018 prototype | `gob` 0.1.0 |
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
| time | `chrono` | 0.4.45 | MIT/Apache | Time zones and formatting; Go layouts are implemented by hand on top | `time` |
| errors | `thiserror` | 2.0.21 | MIT/Apache | Typed errors so callers can match kinds like Go's `errors.Is` | — |
| base64 | `base64` | 0.23.1 | MIT/Apache | Redis category encoding | — |
| schollz/progressbar | `indicatif` | 0.18.6 | MIT | Playground import progress only | — |
| samber/lo, golang-set, copier, sortutil, strutil | std | — | — | Small helpers; written inline | — |

Test-only: `tempfile` 3.27.0. Storage tests use the real backends from
`storage/docker-compose.yml`, never mocks.

## Not adopted from the plan's proposal

| Proposed | Reason |
|---|---|
| `utoipa` | Emits OpenAPI 3; the contract is OpenAPI 2.0 (C-5) |
| `candle` or `burn` | The default FM is the pure-Go `fm.go` on `common/nn`; porting that keeps blob compatibility and numerics (C-4, D-8) |
| `figment` or `config` | Viper's merge rules must be reproduced exactly; a generic crate adds different rules |
| `fred` | `redis` covers standalone, cluster and raw module commands with a larger user base |
| `async-openai` | Would change which request fields survive the master's proxy |
