# gRPC contract

The `.proto` files in `proto/` are byte-identical copies of `protocol/*.proto`
(`SHA256SUMS`). All messages are in package `protocol`, so the full service names
are `protocol.Master`, `protocol.DataStore`, `protocol.CacheStore`,
`protocol.VectorStore` and `protocol.BlobStore`. This file records what the
`.proto` files leave implicit: who serves and calls each service, and the
semantics the Go handlers give to each message.

## Who serves what

Only the master listens for gRPC (`master.host:master.port`, default
`0.0.0.0:8086`). One `grpc.Server` carries every service (`master/master.go`
`Serve`):

| Service | Registered | Called by |
|---|---|---|
| `Master` | always | servers and workers, every `master.meta_timeout` (default 10 s) |
| `DataStore` | always | servers and workers when `database.data_store` starts with `sqlite://` |
| `CacheStore` | always | servers and workers when `database.cache_store` starts with `sqlite://` |
| `VectorStore` | always | servers and workers when `database.vector_store` starts with `xvec://` |
| `BlobStore` | only when `blob.uri` contains no `://` (a local directory) | workers, which then read models through the master |

A worker or server decides whether to proxy from the `database` section of the
config it receives in `Meta.config`, not from its own file (`server/server.go`
and `worker/worker.go` `Sync`). It reconnects whenever the store URL or table
prefix changes between two `GetMeta` calls.

Server options: `grpc.MaxSendMsgSize(math.MaxInt)` and, when
`master.ssl_mode` is set, mutual TLS from `master.ssl_ca`/`ssl_cert`/`ssl_key`:
the master requires a client certificate signed by the CA, and clients verify the
master's certificate against the same CA (`common/util/tls.go`, grpc-go
`advancedtls` with `CertVerification`, so host names are not checked). Client
options: plain text unless the server or worker was started with all three of
`--ssl-ca`, `--ssl-cert`, `--ssl-key` (giving only some is fatal); workers also set
`MaxCallRecvMsgSize(512 MiB)`. The receive limit on the master stays at the gRPC
default of 4 MiB, so a single request (for example `AddVectors` or `UploadBlob`
chunks) must stay under it.

## Errors

Handlers return plain Go errors, which grpc-go sends as status `UNKNOWN` with
`err.Error()` as the message. The exceptions are:

- `VectorStore.DescribeCollection` returns `NOT_FOUND` for a missing collection;
  the Go client maps `NOT_FOUND` back to `storage.ErrNotFound`.
- `CacheStore.ScanScores` wraps a scan error as `INTERNAL` `failed to scan scores: ...`.
- `DataStore.GetItem` and `GetUser` return **OK with the optional field unset**
  for a missing record; the Go client turns that into `storage.ErrNotFound`.
- `CacheStore.Get` returns OK with `value` unset for a missing key; the client
  turns that into a non-existing `ReturnValue`.

A Rust client must accept exactly these shapes, and a Rust server must produce
them, including the status code for every other error (`UNKNOWN`).

## Master service

`GetMeta(NodeInfo) -> Meta` (`master/rpc.go`):

1. Upserts the caller into the meta store `nodes` table with `Type =
   NodeType.String()` (`Server`, `Worker`, `Client`), `Version =
   binary_version`, `Hostname`, `UpdateTime = now (UTC)`.
2. Returns:
   - `config`: `json.Marshal(config.Config)` of the master's live config. Field
     names are the Go struct field names, `time.Duration` values are integers in
     nanoseconds, and secrets are included. A sample is
     `config/meta-config.default.json`; the Rust config crate must both produce
     and parse this exact shape.
   - `collaborative_filtering_model_id`, `click_through_rate_model_id`: ids of the
     current models (0 when none), which are also their blob names.
   - `me`: the caller's own `uuid`, echoed.
   - `workers`, `servers`: uuids of the live nodes of each type (rows newer than
     `master.meta_timeout`).

Node uuids are not random: servers and workers send
`hex(md5(hostname + http_host + decimal(http_port)))` (`ServerName`,
`WorkerName`). `NodeType` `Client` exists in the enum but nothing at the pinned
commit sends it.

`PushProgress(PushProgressRequest)`: workers push their task tracer after each
recommendation round. All entries must carry the same `tracer` or the call fails
with `tracers must be the same, expect <a>, got <b>`; the master keeps the latest
list per tracer for `GET /api/dashboard/tasks`. `status` is one of `Pending`,
`Running`, `Complete`, `Suspended`, `Failed`; `start_time` and `finish_time` are
Unix milliseconds. The `error` field is neither sent by `EncodeProgress` nor read
by `DecodeProgress` (`common/monitor/progress.go`).

## DataStore service

A thin RPC wrapper over `data.Database` (`storage/data/proxy.go`). Conventions:

- `labels` fields are `bytes` holding `json.Marshal(labels)`. A Go nil is the
  four bytes `null`. Items and users with empty `labels` bytes are rejected by the
  Go server (`json.Unmarshal` of empty input fails); feedback tolerates empty
  `labels`.
- Timestamps are `google.protobuf.Timestamp`. `update_at` and `updated` are
  optional: a missing value means the zero time.
- `Init` and `Purge` have no RPC; the Go proxy client refuses them locally.
- Streams (`GetUserStream`, `GetItemStream`, `GetFeedbackStream`) send one
  response per database batch of `batch_size` records, then end with OK.
- `GetFeedbackStream` dereferences `scan_options` without a nil check, so a
  request without `scan_options` panics the Go master. Go clients always send it.
  A Rust client must also always send it.
- The Go client never sends `GetItemStreamRequest.time_limit` even when its
  caller passes one (`ProxyClient.GetItemStream`): the time limit is silently
  dropped in proxy mode. The Go server honours it if present.

## CacheStore service

A wrapper over `cache.Database` (`storage/cache/proxy.go`). Values travel as the
already-encoded strings described in `cache-keys.md`; `Init`, `Scan` and `Purge`
are refused locally by the client. `ScanScores` streams every score document's
`(collection, id, subset, timestamp)`.

## VectorStore service

A wrapper over `vectors.Database` (`storage/vectors/proxy.go`). The wire
`Distance` enum is `Unknown = 0, Cosine = 1, Euclidean = 2, Dot = 3`; the Go
in-memory enum starts at `Cosine = 0`, and the proxy converts. Dense vectors are
sent either as `values` (FP32) or `h_values` (FP16 bits, little-endian, two bytes
per element); sparse vectors use `indices` plus `values`.

## BlobStore service

Served by `blob.MasterStoreServer` over the directory `blob.uri`
(`storage/blob/blob.go`):

- `UploadBlob`: client stream of chunks of at most 4 MiB. Every chunk repeats
  `name` and `timestamp`; a chunk with a different name or timestamp fails the
  upload (`inconsistent name`, `inconsistent timestamp`). Data goes to a temp file
  `upload-*` in the same directory and is renamed to `name` at the end.
- `DownloadBlob`: server stream of chunks of at most 4 MiB.
- `ListBlobs`: names of the regular files in the directory, so an in-progress
  `upload-*` temp file is listed too (the master ignores names that do not parse
  as integers).
- `RemoveBlob`: deletes the file.

Names are joined to the directory with `path.Join` and are not sanitised: the Go
server accepts `..` and `/`. See `PORTING.md` decision D-12 for how the port
handles this.

## Cluster coordination outside the protos

These are not RPCs but every node must compute them identically for a mixed
Go/Rust cluster to work.

- **User partitioning between workers** (`worker/worker.go` `pullUsers`): each
  worker builds a consistent-hash ring from `Meta.workers` with
  `github.com/lafikl/consistent`: 10 points per worker at
  `hash(worker_uuid + decimal(i))` for `i` in `0..10`, where `hash(s)` is the
  first 8 bytes of BLAKE2b-512(`s`) read as little-endian `u64`. A user belongs to
  the first ring point `>= hash(user_id)`, wrapping to the first point. A worker
  whose own uuid is not in `Meta.workers` processes nobody.
- **Model ids**: `time.Now().UnixMilli()` on the master when training ends. A
  worker pulls a model when the id in `Meta` is greater than the one it holds.
