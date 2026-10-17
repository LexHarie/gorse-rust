# Porting notes

Go behaviour the port reproduces even where it looks wrong, open questions, and
the few deliberate deviations. Every entry cites the Go source at `add21b5`.
Entries marked **observed** were also seen on running Go binaries during Phase 0.
Deviations point to the decision in `PORTING.md` that approves them.

## Quirks reproduced

### Wire and HTTP

- **Q-1 Cache time values carry Go's monotonic clock suffix.** `cache.Time` stores
  `t.String()`, so a value from `time.Now()` ends in ` m=+<seconds since process
  start>` and uses the writer's local zone abbreviation
  (`storage/cache/database.go`). Readers parse with `dateparse.ParseAny`, which
  ignores the suffix. Rust writers emit the same shape: zone as the local
  abbreviation, nine fractional digits trimmed like Go, and the `m=+` offset since
  process start. **observed**: `2026-10-17 02:58:44.902094534 +0000 UTC m=+39.779705272`.
- **Q-2 JSON responses are one-space indented with no trailing newline**
  (go-restful `WriteAsJson` defaults, `server/rest.go` `Ok`). **observed**
- **Q-3 Router errors are go-restful's plain text** `404: Page Not Found` and
  `405: Method Not Allowed` (with `Allow`), without `X-Request-ID`. **observed**
- **Q-4 `Last-Modified` uses Go's `time.RFC1123`, which prints `UTC`, not the
  `GMT` HTTP requires** (`server/rest.go` `SetLastModified`).
- **Q-5 Two dashboard routes look up the cache key `time.Now().String()`** for
  `Last-Modified` (`master/rest.go` `getLatest`, `getExternal`). The key never
  exists, so the header is always `Mon, 01 Jan 0001 00:00:00 UTC`.
- **Q-6 `/api/dump` appends the JSON stats after the binary end marker** and keeps
  `Content-Type: application/octet-stream` (`master/rest.go` `dump`).
  **observed**; format in `contracts/model-blobs.md`.
- **Q-7 Dump and restore drop record metadata.** Users and items are dumped
  without `update_at` and feedback without `updated`, so a restore resets them.
- **Q-8 The OpenAPI document's `required` arrays come out in random order** from
  go-restful-openapi. The Rust nodes serve the frozen file, whose order is one of
  those orders; clients do not depend on it.
- **Q-9 `Meta.config` carries every secret** (database URLs with passwords, API
  keys, OIDC client secret) in plain JSON to every node that calls `GetMeta`
  (`master/rpc.go`).

### gRPC proxies

- **Q-10 `ProxyClient.GetItemStream` never sends `time_limit`**, so a worker or
  server in proxy mode silently scans all items (`storage/data/proxy.go`).
- **Q-11 Items and users with empty `labels` bytes are rejected by the Go proxy
  server** (`json.Unmarshal` of empty input); feedback accepts empty labels. A
  Go nil is sent as `null`.
- **Q-12 Missing records come back as OK with the optional field unset**
  (`GetItem`, `GetUser`, `CacheStore.Get`), not as `NOT_FOUND`; only
  `VectorStore.DescribeCollection` uses `NOT_FOUND`. Every other error is
  `UNKNOWN` with the Go error text.
- **Q-13 Progress `error` is never transmitted**: `EncodeProgress` does not set it
  and `DecodeProgress` does not read it (`common/monitor/progress.go`).
- **Q-14 `BlobStore.ListBlobs` lists in-progress `upload-*` temp files**
  (`storage/blob/blob.go`); the master skips names that are not integers.

### Models and blobs

- **Q-15 Model hyper-parameters change type across a master restart.**
  `meta.Model.Params` is stored as JSON, so after a restart every number is a
  `float64`. `Params.GetInt` accepts only `int`, so integer parameters
  (`NEpochs`, `NFactors`, `BatchSize`, ...) of a reloaded model fall back to
  their defaults, while `GetFloat32` accepts `float64` and keeps float
  parameters (`model/params.go`, `storage/meta/database.go`). `maps.Equal` in
  `Model.Equal` then also sees `int(3) != float64(3)`. The Rust `Params` keeps the
  dynamic type of each value so both effects are reproduced.
- **Q-16 Type-mismatch log lines name the wrong type**: they log
  `reflect.TypeFor[ParamName]().Name()`, always `ParamName`, instead of the
  value's type (`model/params.go`).
- **Q-17 Blob bytes are not deterministic**: user embeddings and FM scalers are
  written in Go map order (`logics/cf.go`, `model/ctr/fm.go`).
- **Q-18 XLA and non-XLA CTR blobs share the `AFM`/`AFM2` header but differ in the
  parameter section**, so the two Go builds cannot read each other's blobs
  (`model/ctr/fm.go`, `fm_xla.go`).
- **Q-19 `encoding.ReadBytes` of a zero-length value depends on the reader**
  (`common/encoding/encoding.go`): it calls `Read` with an empty buffer, which
  `bytes.Buffer` and `os.File` answer with `0, nil` and `bytes.Reader` at the end
  of its data answers with `EOF` (observed with a probe test). Not reproduced:
  no blob layout ends with a string (the CF blob ends with a slice, the CTR blob
  with tensors), so it cannot change a result, and the Rust `read_bytes` always
  returns the empty value.

### Storage

- **Q-20 Timestamp precision differs per backend**: MySQL `datetime` keeps whole
  seconds for `time_stamp`, Postgres and SQLite microseconds, MongoDB
  milliseconds (`contracts/schema/samples`).
- **Q-21 `users.labels` defaults to `'[]'` on MySQL and Postgres but `"null"` on
  SQLite** (`contracts/schema/data`).
- **Q-22 Full-text search columns are sorted lexicographically by their SQL
  fragment**, not kept in config order (`storage/data/sql.go` `Reconcile`).
- **Q-23 The Postgres cache store runs `CREATE EXTENSION IF NOT EXISTS btree_gin`
  at `Init`** and fails for roles without that privilege.
- **Q-24 `database.vector_table_prefix` is applied only by xvec**; the Qdrant,
  Milvus and Weaviate constructors ignore it (`storage/vectors/*.go`).
- **Q-25 The SQLite stores need the `json_contains` function registered on every
  connection** by the process (`storage/scheme.go`); the database file alone
  cannot run Gorse's category queries.

### Nodes and command line

- **Q-26 `config.MkDir` creates `$HOME/.gorse/var/lib/master` while flags are
  defined**, so even `gorse-master --help` creates it. A non-default
  `--cache-path` is not created, and the master then exits with `failed to init
  meta database: unable to open database file (14)`. **observed**
- **Q-27 Node uuids are `md5(hostname + http_host + http_port)`**
  (`ServerName`, `WorkerName`), so two nodes started with the same host and port
  on one machine register as the same node.
- **Q-28 A worker whose uuid is not yet in `Meta.workers` processes no users**
  that round (`worker/worker.go` `pullUsers` returns `current node isn't in worker
  nodes`).
- **Q-29 The env binding `GORSE_DASHBOARD_AUTH_SERVER` (`master.dashboard_auth_server`)
  has no config field** at the pinned commit (`config/config.go`), so setting it
  has no effect. Listed in `contracts/config/config.keys.md`.
- **Q-30 The playground mode overwrites `$HOME/.gorse/etc/config.toml` on every
  start** and, for `ml-100k`, shifts feedback timestamps by
  `now - 1998-04-22T23:10:38Z` (`cmd/gorse-in-one/main.go`).
- **Q-31 Dashboard session cookies use keys generated at master start**, so every
  restart logs everyone out (`master/rest.go` `cookieHandler`). The Rust master
  does the same; no cross-implementation cookie compatibility is needed.

## Deviations

- **X-1 xvec on-disk format** (`PORTING.md` D-6). The Rust master implements the
  `xvec://` store natively with its own file format. Workers and servers never
  open xvec files (they use the master's `VectorStore` service), so a mixed
  cluster is unaffected; switching a master between Go and Rust on the same
  `xvec://` directory makes it rebuild the collections on its first training
  round.
- **X-2 BlobStore names are checked** (`PORTING.md` D-12). The Go server joins
  the requested name to its directory without checks, so `../` escapes it. The
  Rust server rejects names that are empty, contain `/` or `\`, or are `.` or
  `..`, with `UNKNOWN` `invalid blob name`. Every name Gorse itself uses is a
  decimal model id.
- **X-3 `GetFeedbackStream` without `scan_options`** (`PORTING.md` D-13). The Go
  master dereferences the nil field and the process crashes. The Rust master
  answers `UNKNOWN` `scan_options is required`. Go clients always send it.
- **X-4 `/debug/pprof/*`** (`PORTING.md` D-14). A Rust process cannot produce Go
  runtime profiles; the Rust nodes answer these paths with `404: Page Not Found`.
- **X-5 No `go_*` Prometheus metrics.** The Go client registers Go runtime
  collectors (`go_goroutines`, `go_memstats_*`, ...); the Rust nodes export the
  same Gorse metrics plus `process_*` but no `go_*` series.
- **X-6 `--help` layout.** Flags, defaults and behaviour match
  `contracts/cli/*.help.txt`; the help text is laid out by clap, not cobra.
- **X-7 Corrupt lengths are errors** (`PORTING.md` D-23). Go's `ReadBytes` and
  `ReadSlice` panic on a negative `i32` length (`make` with a negative size) and
  allocate any positive length before reading the data; `WriteBytes` and
  `WriteSlice` truncate a length above `i32::MAX` and write a corrupt stream; a
  gob map count makes Go ask the runtime for a map of that size before reading
  any entry. The Rust functions return an error for the first three and grow
  buffers and maps only as data arrives. Well-formed input reads the same.
- **X-8 gob values Gorse never writes** (`PORTING.md` D-23). Go's gob decoder
  accepts any gob-registered type inside `model.Params` (`uint`, `[]string`, ...)
  and any bytes in a string. The Rust decoder reads the types Gorse writes
  (`contracts/model-blobs.md`) and returns `Unsupported` for the rest; strings,
  including `ReadString` values such as user ids in the CF blob, must be UTF-8.
  The two cases where Go returns a value and Rust does not are listed in
  `crates/gorse-common/tests/gob_golden.rs` (`UNSUPPORTED`). Ids reach Gorse
  through JSON, which Go decodes to UTF-8, and protobuf, which rejects other
  strings.

## Open questions

- **O-1** The Go CI's model tests read the datasets from `https://cdn.gorse.io`,
  which this environment's network policy blocks. Phase 3 metric parity needs
  them (`PORTING.md` status board).
