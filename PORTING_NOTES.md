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

### Numerics

- **Q-32 `MM` adds the product into `c`, except `a·bᵀ`, which overwrites it**
  (`floats_avx.c`, `floats_avx512.c`, `floats_neon.c` and `mm.go`: the
  `!transA && transB` branch assigns `c[i*ldc+j] = dot(...)`, the others use
  `+=`). `common/nn` (`tensor.go`, `matMul` and `batchMatMul`) always multiplies
  into a fresh zeroed result, so there both read as plain products.
- **Q-33 Results depend on the CPU and on `GODEBUG`.** `Dot`, `Euclidean`, `MM`,
  `MulConstAdd` and `MulConstAddTo` round differently at the AVX-512, AVX and
  scalar levels (the AVX-512 kernels use FMA and 16-wide partial sums, the AVX
  kernels 8-wide sums); `crates/gorse-common/tests/data/floats_golden.json` has
  the hashes per level. Two Go nodes on different CPUs disagree in the last bits.
  The port picks the level Go picks on the same CPU and `GODEBUG`
  (`cpu.avx512f=off`, `cpu.fma=off`, `cpu.avx=off`, `cpu.all=off`, read as
  `golang.org/x/sys/cpu` reads them), so it computes what a Go node on that
  machine computes.
- **Q-34 Go's scalar `MM` needs equal leading dimensions** where the kernels do
  not: `mm.go` cuts whole rows and combines them with `MulConstAdd` and `Dot`,
  which panic with `floats: slice lengths do not match` unless `ldb == ldc`
  (`lda == ldb` for `a·bᵀ`). The port panics the same way at the scalar level.

### Collections

- **Q-35 `PriorityQueue.Pop` keeps the value in the lookup set.** `Push`
  ignores any value pushed before, so a value popped from a queue can never be
  pushed into it again (`common/heap/pq.go:81-93`; `Pop` at `pq.go:96-99`
  never touches `lookup`). Covered by `popped_values_are_not_pushed_again` and
  the golden `bounded at 10` cases (the `ann.Bruteforce` push-then-pop pattern).
- **Q-36 `PriorityQueue.Clone` does not copy the lookup set.** The copy starts
  with an empty set (`pq.go:117-122`), so pushing a value it already holds adds
  a second element with that value. `HNSW.searchLayer` clones its enter points
  this way. Covered by `clone_queue` and the golden cases.
- **Q-37 `PriorityQueue.Unmarshal` trusts its input.** Any non-zero order byte
  means descending (Go's `binary.Read` into a `bool` is `b != 0`); the heap
  array is taken as written, without checking heap order or NaN weights;
  duplicate values stay in the array while the lookup set holds each once.
- **Q-38 `TopKFilter` accepts NaN weights.** Only `PriorityQueue.Push` rejects
  NaN. Weights compare with Go's `<` and `>`, false for NaN, so where a NaN ends
  up depends on the push order; the port compares with the same operators, not
  `total_cmp`.
- **Q-39 `PriorityQueue.Pop`'s doc comment says an empty queue returns an
  error; it panics** (index out of range in `container/heap.Pop`). See X-10.

### Consistent hashing

These are in `lafikl/consistent` (`v0.0.0-20220512074542-bdd3606bfc3e`), which
places users on workers (`contracts/grpc.md`); the golden data
(`consistent_golden.json`, `contracts/tools/consistentgolden`) checks each one.

- **Q-40 A 64-bit hash collision leaves a point without a host.** `Add`
  overwrites the colliding point's host (last added wins) while the sorted set
  keeps both copies; removing either host deletes the shared map entry and one
  copy. `Get` landing on the orphaned point returns `""` and `GetLeast` panics
  (a nil pointer dereference in `loadOK`). Checked against Go on states written
  straight into the ring's fields (golden `injected`), since the real hash
  cannot be made to collide.
- **Q-41 `GetLeast` wraps at the number of distinct points** (`len(c.hosts)`),
  not the length of the sorted set; they differ only after a collision, when
  the last point is skipped.
- **Q-42 Reads that write the total load.** `MaxLoad` turns a total load of 0
  into 1, and `loadOK` (inside `GetLeast`) turns a negative total into 0, both
  under Go's read lock (a data race in Go). `max_load` and `get_least` take
  `&mut self`; the random sequences check the total after every call.
- **Q-43 `Remove` keeps the removed host's load in the total** and always
  returns `true`, also for a host that was never added.
- **Q-44 `MaxLoad` on an empty ring panics** (integer divide by zero), after
  setting the total to 1.
- **Q-45 `Inc`, `Done` and `UpdateLoad` ignore unknown hosts**, and `Done`
  drives a load below zero. Load arithmetic wraps on overflow as Go's `int64`
  does.

### Feedback type expressions

`common/expression`, checked by `expression_golden.json`
(`contracts/tools/exprgolden`).

- **Q-46 `=` parses and matches every value.** The pattern accepts `=` and the
  switch maps it to `None` (`expression.go:84-95`): `a=5` gives type `a`,
  `None`, value 5, so `Match` ignores the value, `String` prints `a` and `ToPB`
  still sends 5.
- **Q-47 A value without an operator is parsed and then ignored.** `a.5` gives
  type `a`, `None`, value 0.5; the type takes digits greedily, so `a1.5` gives
  type `a1` and value 0.5, and `a1.` fails on the value `.`.
- **Q-48 `FromString` is not atomic and keeps the old value.** The type and
  operator are written before the value is parsed; a bad value then sets the
  value to what `strconv.ParseFloat` returned (0 for `invalid syntax`, `+Inf`
  for `value out of range`) and returns the error. Text without a value leaves
  the previous value in place. With `encoding/json` decoding into the elements
  already in a slice, this carries a stale value across a config override
  (`["like"]` over a file value `star>=3` gives `{like, None, 3}`; only `ToPB`
  shows it). `master.applyRecommendOverride` decodes `RECOMMEND_CONFIG` into
  `cfg.Recommend` that way (for WP-1-15 and WP-4-3).
- **Q-49 Values of 1e6 and more, or below 1e-4, do not survive a round trip.**
  `String` formats the value with `%v`, which switches to exponent form outside
  `[1e-4, 1e6)` (`read>=1e+06`), and the pattern has no exponent. A config
  holding such a value marshals, but unmarshalling it back fails. `postConfig`
  stores the marshalled `Recommend` in the meta store and
  `applyRecommendOverride` reads it back on start (error logged,
  `master.go:328`) and on reload (`master.go:183`, which then fails); inferred
  from the call sites, for WP-4-3 to confirm.
- **Q-50 `MarshalJSON` escapes `<`, `>` and `&`** (`"test\u003c16"`, asserted by
  the Go test). The serde `Serialize` writes the plain string; matching Go's
  bytes in a whole document needs a writer that escapes every string like Go,
  which every go-restful response also needs (REST work packages).
- **Q-51 `FromPB` keeps `ExprType` for an unknown enum number** (Go's `switch`
  has no default).
- **Q-52 `MarshalJSON` has a pointer receiver.** A value that is not addressable
  (passed by value, a map value) encodes as the default struct
  `{"FeedbackType":"read","ExprType":4,"Value":3}`; slice elements and pointers
  use the string form. Every Gorse field is a slice, so the Rust `Serialize`
  always writes the string form.
- **Q-53 JSON `null` is an error inside a slice but fine for the slice.**
  `UnmarshalJSON("null")` parses the empty string and fails; a `null` slice is
  set to nil without calling it. serde's `Vec<T>` rejects `null`, so the config
  types (WP-1-15) need `Option` or a default there.

### Vector indexes

- **Q-54 `Bruteforce.Add` returns the new length; `HNSW.Add` returns the new
  vector's index** (`bruteforce.go:36`, `hnsw.go:70`). Only the HNSW's value is
  used (`logics.MatrixFactorizationItems.Add` stores the item id there).
- **Q-55 `HNSW.SearchIndex` finds the query vector itself; `Bruteforce` leaves
  it out.** With a distance that is zero for equal vectors the HNSW's first
  result is the query (unless `prune0` drops it); with Gorse's `-dot` distance
  it lands wherever its own score puts it.
- **Q-56 An HNSW grows by at most one layer per insert.** A vector drawn at
  level `l` above the top layer is linked into the existing layers and then
  becomes the only member, and enter point, of one new layer, however large
  `l` is (`hnsw.go:188-193`).
- **Q-57 Layer-0 neighbour lists are cut to `maxConnection` (48), not
  `maxConnection0` (96).** The check uses 96 at layer 0, but the shrink keeps
  48 (`hnsw.go:177-180`, against its own comment), so a list that reaches 97
  falls back to the 48 nearest.
- **Q-58 Layers come from Go's global `math/rand`.** The level is
  `int(math32.Floor(-math32.Log(rand.Float32()) * levelFactor))`, drawn after
  the enter point's distance is computed; the first insert into a fresh index
  draws nothing. The global source is randomly seeded, so Go builds a different
  graph on every run and no stream is reproduced; `Hnsw::with_uniform` takes
  the draws, which the golden tests replay from Go. A draw of exactly 0 gives
  `int(+Inf)`, which is `MinInt64` on amd64, and the insert panics with
  `index out of range [-2]` (checked against Go with the draw stubbed to 0).
- **Q-59 `Unmarshal` takes the level factor, `maxConnection`,
  `maxConnection0`, `ef` and `efConstruction` from the blob**, not from
  `NewHNSW`; an `ef` above zero then sets the search breadth (`efSearchValue`).
- **Q-60 Go writes each upper layer's entries in `sync.Map` order**, which
  changes from run to run, so two Go blobs of the same graph differ in bytes.
  The Rust `marshal` writes them by vector index, one of Go's orders; the
  goldens compare Go's blob with its entries sorted.
- **Q-61 Searching an empty HNSW panics** (`index out of range [0] with length
  0`: there is no enter point); an empty `Bruteforce` returns nothing, and
  `SearchIndex` on either returns `index out of range: q`.
- **Q-62 `Unmarshal` accepts layers it cannot search.** A negative entry count
  reads as no entries; a vector stored twice in a layer keeps the later queue
  and can leave another vector missing, so a search reaching it dereferences
  nil; an enter point outside the vectors panics on the first search. Rust
  reads the same and panics in the same searches.
### Time and dates

Go's `time` (go1.27.0) as `gorse_common::gotime` and `araddon/dateparse`
(6b43995a97de) as `gorse_common::dateparse`, checked by `gotime_golden.json` and
`dateparse_golden.json` (`contracts/tools/timegolden`, `PORTING.md` D-35).
gotime is a line-by-line port; these are the Go behaviours Gorse can observe.

- **Q-63 `Time.String()` ends in ` m=±<seconds>` for times from `now()`** (Q-1).
  The reading counts from the process clock start; `Round(0)`, `In`, `Local`
  and `UTC` drop it, as in Go.
- **Q-64 Malformed zone data under `$ZONEINFO` is not reported.**
  `LoadLocation` assigns the read's nil error, not the parse's, to `firstErr`
  (`time/zoneinfo.go`), so a corrupt file there falls through to the system
  sources and the final error is theirs.
- **Q-65 `$ZONEINFO` is read once per process**, at the first `LoadLocation`;
  `Local` is read once from `$TZ` (else `/etc/localtime`, else UTC named
  "UTC").

dateparse (each is covered by the golden corpus or the ported tests unless
marked):

- **Q-66 Indices are byte positions**; after a multi-byte character the index
  sits on its last byte. The time-state loop reads raw bytes as characters, so
  byte 0xC3 counts as the letter `Ã`; the digit-slash check reads the byte after
  the slash the same way (not covered by the corpus).
- **Q-67 Restarts drop the options.** `"Mon "`, `"Tue "` and the other weekday
  prefixes restart the parse without the caller's options (not covered by the
  corpus); st/nd/rd/th suffixes are removed by a restart; a comma in the time
  becomes a period by a restart; a colon before milliseconds is rewritten to a
  period in the input itself; `"jan."` becomes `"jan"` only when the period is
  at index 4.
- **Q-68 Lowercasing is Go's `unicode.ToLower`:** `İ` lowers to `i` and the
  Kelvin sign to `k`, so `"AprİL 8, 2009"` yields the layout `"JanuaryL…"` and
  fails.
- **Q-69 Each `parse()` re-applies the full month name and the weekday skip** to
  the layout.
- **Q-70 `RetryAmbiguousDateWithSwap`** runs on whichever parser came back
  (including one from a restart without options), re-parses the final input in
  `Local`, replaces the parser even with nil while keeping the original error,
  and looks for "month out of range" anywhere in the error text.
- **Q-71 An "AM" suffix is written into the layout as "PM".**
- **Q-72 All-digit inputs:** 13- and 16-digit timestamps wrap on overflow;
  lengths 14, 8 and 4 become fixed layouts whatever the digits; under 4 digits
  gives "too short" with the raw bytes.
- **Q-73 `ParseFormat` can return invalid UTF-8.**
- **Q-74 "TZ offset not recognized"** fires for offsets of 0, 1, 2 or 4
  characters and quotes the local copy of the input.
- **Q-75 An unknown period-month** returns a non-nil parser together with the
  error.

The Go tests are ported case for case: `time/format_test.go` →
`tests/gotime_go_format.rs`, `time_test.go` and `mono_test.go` →
`tests/gotime_go_time.rs`, `zoneinfo_test.go`, `zoneinfo_unix_test.go` and
`abs_test.go` → `tests/gotime_go_zoneinfo.rs`, the 39 `example_test.go` examples
with an `// Output:` comment → `tests/gotime_go_example.rs` (the 7 without one
are not run by `go test` either), dateparse's `parseany_test.go` →
`tests/dateparse_go.rs`, plus `storage/cache/database_test.go` lines 92-99.
Not ported: `TestCountMallocs` and `TestUnmarshalTextAllocations` (allocation
counts need an `unsafe` counting allocator, and Rust's allocations are not Go's
escape analysis), the timer and ticker tests (gotime has no timers; Gorse uses
tokio's), `TestLinkname` (linkname shims), `TestEmbeddedTZData` (X-27) and the
android and windows files. Harness differences: tests that set environment
variables run as child processes (`set_var` is `unsafe` in Rust 2024), one
child per `$TZ` value in `TestEnvTZUsage`; `ForceUSPacificForTesting` and the
dateparse tests load zones from the stand-in GOROOT zip (D-34); `testing/quick`
becomes a fixed-seed splitmix64 with Go's counts; `TestFormatAndParse` adds an
explicit `f(0)` because Go's property only ever checks `sec == 0`;
`TestTimeGob` round-trips `MarshalBinary`/`UnmarshalBinary` without gob framing;
dateparse's `TestPreferMonthFirst` and `TestRetryAmbiguousDateWithSwap` run in
America/Denver, which `TestInLocation` leaves behind when Go runs the file in
order.

### Float32 math

`github.com/chewxy/math32` v1.11.2 as `gorse_common::math32`, checked by
`math32_golden.json` (`contracts/tools/math32golden`): 13 functions over 21 862
inputs, each recorded from the default amd64 build (`asm`, where math32's `Exp`,
`Log` and `Sqrt` are assembly) and from a `-tags noasm` build (`noasm`, the Go
code), plus FNV-1a digests of 10 unary functions and the two conversions over
all 2^32 inputs per column, recomputed by the ignored test
`math32_exhaustive`. Only what Gorse calls is ported (X-34).

- **Q-76 amd64 `Exp` adds the exponent into its polynomial.** `exp_amd64.s` has
  `ADDSS $T4, X1` where it means to load `T4` into `X1`, which still holds
  `float32(k)`. Results are off by a few ulps, by more as `|x|` grows.
- **Q-77 amd64 `Exp` range.** `k` is `x·log2(e)` rounded half to even
  (`CVTSS2SL`), and the result is assembled as `2^k` times the polynomial
  without the Go code's range checks. So `Exp` is `+Inf` from `x = 88.37627` on
  (the true value is finite up to 88.72; the Go code gives `3.3259769e38` at
  88.7), subnormal only from -87.33655 to -87.68312 and 0 from `x = -87.68313`
  down (the Go code has subnormals to about -104), and 0 for every finite `x`
  with `|x·log2(e)| >= 2^31` (from about `±1.49e9`), where the conversion gives
  `MinInt32`: `Exp(1e9)` is `+Inf` but `Exp(2e9)` is 0, while `Exp(+Inf)` is
  `+Inf`. Asserted in `math32_amd64_differs_from_go_code`.
- **Q-78 amd64 `Log` takes the exponent bits raw.** `k = e - 126` with no
  normalisation, so subnormal inputs give wrong results (`Log(1e-40)` is
  `-88.02122`, the Go code gives `-92.10341`).
- **Q-79 amd64 `Log` halves when `f1 <= √2/2`** (`CMPSS X2, X0, 5` computes
  `!(HSqrt2 < f1)`), where the Go code halves when `f1 < Sqrt2/2`. They take
  different paths for mantissa `0x3504F3`, and the results differ at some
  exponents: `Log(0.70710677)` is `0xBEB17218` from the assembly and
  `0xBEB17219` from the Go code. The golden covers that mantissa and its
  neighbours at every exponent.
- **Q-80 amd64 `Log` of any negative input**, a negative NaN included, returns
  math32's NaN `0x7FE00000`; a positive NaN comes back unchanged.
- **Q-81 NaN bits.** math32's `NaN()` is `0x7FE00000`, not Rust's `0x7FC00000`.
  `Sqrt` on amd64 is `SQRTSS`: `0xFFC00000` for a negative input and the input
  quieted for a NaN; the Go code returns `0x7FE00000`. The bits feed `Pow` and
  `Tanh`, and any NaN a caller stores.
- **Q-82 `Pow` and `Tanh` call `Exp`, `Log` and `Sqrt`**, so on amd64 they
  inherit Q-76 to Q-81. `Pow`'s `isOddInt` and its loop over the bits of `y`
  convert with `int32(x)`, so they get amd64's out-of-range result (Q-84).
- **Q-83 `Sin` and `Cos` above 2^29.** math32's `trigReduce` adapts the float64
  Payne–Hanek code with 32-bit shift counts over the 64-bit digits of 4/π, takes
  the octant as `hi >> 29` of a 64-bit value and masks it to 0..7 only when it
  is odd. `Sin` and `Cos` then pick the polynomial and sign from that unmasked
  octant, so results for `|x| >= 2^29` are not the true sine. Reproduced line by
  line; the exhaustive digests cover every such input.
- **Q-84 Go's float-to-integer conversions.** `int32(x)` and `int(x)` truncate
  toward zero; for NaN and out-of-range values amd64 gives `MinInt32` and
  `MinInt64` (`CVTTSS2SL`, `CVTTSS2SQ`), where Rust's `as` saturates and maps
  NaN to 0. `math32::go_f32_to_i32` and `go_f32_to_i64` reproduce amd64; the
  exhaustive digests check them on every float32.

### expr-lang parser

`github.com/expr-lang/expr` v1.17.8's lexer and parser as `gorse_expr`, checked by
`parser_golden.json`, `parser_bulk_golden.json` and `parser_go_tests.json`
(`contracts/tools/exprparsegolden`; Unicode tables from
`contracts/tools/exprunicode`). Gorse calls only `parser.Parse`
(`config/config.go:975`), whose error decides `item_expr` validity. Each quirk is
in the golden data (`quirks` source of `parser_golden.json` unless said
otherwise). Decisions D-36 to D-45 are in `PORTING.md`.

Each is in the golden data (`quirks` source of `parser_golden.json` unless
said otherwise), so the Rust result is Go's.

- **Q-85 `\U` escapes wrap Go's 32-bit runes.** The unescaper shifts
  eight hex digits into an `int32`; from `\U80000000` up the value is
  negative, passes the `> MaxRune` check and is written as its low byte:
  `"\UFFFFFFFF"` is the one byte `0xFF` (printed `"\xff"`), `"\U80000041"` is
  `"A"`. `\U7FFFFFFF` and `\U00110000` fail ("unable to unescape string").
  Surrogates (`"\uD800"`, `"\u{D800}"`) become U+FFFD.
- **Q-86 A lexer error replaces an earlier parser error.** The lexer
  runs lazily, one token ahead of the parser; the parser's own errors are
  first-wins, but a lexer error met while the parser keeps reading after its
  error overwrites it. `count(, 1)` fails with `unexpected token
  Operator(",") (1:7)`, `count(,"\q")` with `invalid char escape (1:10)`.
- **Q-87 The `if` form's node is not counted and has no location.**
  `parseConditionalIf` builds the `ConditionalNode` directly instead of
  through `createNode`: it is outside the node budget and its location is
  `0:0` (`if a { 1 } else { 2 }` walks to `ConditionalNode 0:0`).
- **Q-88 `true`, `false`, `nil` and numbers take no postfix.** They
  return before `parsePostfixExpression`: `true.foo` is `unexpected token
  Operator(".")`, `nil[0]` likewise, and `1.foo` is `bad number syntax:
  "1.f"` (the lexer reads `1.` as a number).
- **Q-89 Map pairs are at the `{`.** Every `PairNode` takes the
  location of the map's opening brace, not of its key.
- **Q-90 A ternary is at the token after it.** The `?:`
  `ConditionalNode` takes the location of the current token after its second
  branch (for `a ? b : c`, the end of input, `8:9`).
- **Q-91 `not` before an operator is matched by value and located at the
  operator.** After `not` the parser checks only the next token's value, so a
  string `"in"` works as the operator: `a not "in" b` is `not (a in b)`. The
  `UnaryNode` takes the location of the `in` token, not of `not`.
- **Q-92 The lexer and the unescaper disagree on escapes.** The lexer
  rejects `\?`, `` \` `` and an escaped quote of the other kind (`'\"'`:
  `invalid char escape`), which the unescaper would accept; it accepts octal
  escapes from `\4` to `\7`, which the unescaper rejects (`"\400"`:
  `unable to unescape string`).
- **Q-93 Go's `Lexer.Reset` keeps the previous positions.** Reusing a
  lexer (or a `parser.Parser`) for a second source starts reading at the
  first one's end. `parser.Parse` and `ParseWithConfig` make a new parser each
  time, so Gorse never sees it; the port has no reuse API (`PORTING.md` D-42).
- **Q-94 Newlines.** A quoted string cannot hold a line feed (`literal
  not terminated`), but a lone carriage return in it becomes a line feed;
  backquoted strings keep `\r` as is.
- **Q-95 Comparison chains and `?:` share operands.** `a < b < c <= d`
  is `a < b && b < c && c <= d`; `a ?: b` is `a ? a : b` (`PORTING.md` D-39).
- **Q-96 An error on an empty line has no position.** `file.Error`
  leaves the snippet empty when the error's line is empty, and then prints
  only the message: `a +\n\n` fails with just `unexpected token EOF`.
- **Q-97 Numbers.** Underscores are removed anywhere before parsing
  (`0x_1` is 1, `1_0` is 10); decimal literals may have leading zeros (`08`
  is 8); the most negative integer cannot be written (`-9223372036854775808`
  is unary minus on an out-of-range literal); a float that overflows is
  `invalid float literal: ... value out of range`, one that underflows is 0.
  The parser's `float literal is too large` and `integer literal is too
  large` checks cannot fire on 64-bit Go.
- **Q-98 Disabling a builtin does not touch the predicate builtins.**
  `Disabled` applies to the builtins table only; `all`, `count`, `map`,
  `filter` and the other predicate functions are still parsed as builtins
  (golden config 7). Functions and environment names override both
  (configs 8 and 9).

### Util, math/rand and TLS

- **Q-99 `Sample` and `SampleInt32` count excluded values outside the
  interval.** The "take everything" branch triggers when `n >= high - low -
  |exclude|`, with every excluded value counted, and then returns every
  allowed value, which can be more than `n`: `Sample(5, 50, 41, {6, 7, 100,
  -1})` returns 43 values (`common/util/random.go`). Covered by Go goldens
  either side of the threshold.
- **Q-100 `NormFloat64`, `ExpFloat64` and Go's `math.Exp` follow the CPU.** On
  amd64 Go's `math.Exp` uses FMA when the CPU has it and gives different bits
  without it; the Rust port detects FMA at run time the same way and has both
  paths checked against Go (`GODEBUG=cpu.fma=off` for the plain one). The
  ziggurat samplers only reach `Exp` and `Log` in their rare rejection
  branches. Go's amd64 `math.Log` does not normalise subnormal inputs; nor
  does the port.
- **Q-101 Go's `NaN` bits.** `ParseFloat("nan")` returns Go's `math.NaN()`,
  bits `0x7FF8000000000001`, not Rust's `f64::NAN`.
- **Q-102 No host name is checked on either side of a TLS connection**
  (`advancedtls.CertVerification`): a certificate for another host is
  accepted.
- **Q-103 `h2` is offered as ALPN on both sides and enforced on neither.**
- **Q-104 A CA limited to server authentication** works in a client's CA file
  and makes a server reject every client: the server checks client
  certificates for client authentication all the way up to the CA.
- **Q-105 `ParseInt` with bit size 1 drops a range error**
  (`ParseInt("-2", 10, 1)` is `-1, nil` in go1.27.0). Gorse never uses bit
  size 1; `gostrconv::parse_int` has the same logic (by reading the code; its
  tests do not exercise that case).

### Storage core and Go library ports (WP-1-14)

- **Q-106 go1.27 `ParseFloat` past 800 digits.** `decimal.set` keeps 800
  digits, but it takes the decimal point from the capped count. A mantissa
  longer than that with a nonzero tail therefore parses wrongly. The bits of
  `ParseFloat(s, 64)`, matched by the port at both `SetOptimize` settings:

  | s | Go bits |
  |---|---|
  | `"9007199254740993" + "0"*790 + "1e-791"` | `41cad7f29abcaf49` (9.007199254740993e+08) |
  | `"9007199254740993" + "0"*900 + "1e-901"` | `2af42c7ff0054685` (9.007199254740992e-102) |
  | `"9007199254740993" + "0"*700 + "1e-701"` | `4340000000000001` |
  | `"0." + "0"*900 + "1e-901"` | `0` |

  A correctly rounding parser gives `4340000000000001` for the first three.
  `gojson` reaches this through any JSON number that long.
- **Q-107 Unhashable `json_contains` elements have two texts.**
  - Inserting into a map, or looking up in a non-empty one, goes through
    `typehash`: `runtime error: hash of unhashable type []interface {}` (or
    `map[string]interface {}`).
  - A lookup in an empty map goes through `mapKeyError`: `hash of unhashable
    type: []interface {}`.

  `-0` and `0` are the same key.
- **Q-108 modernc's SQLite boundary.** This is what Go sees, and the Rust
  SQLite glue must reproduce it:
  - A `TEXT` argument arrives through `libc.GoString`, so it ends at its
    first NUL (`modernc_text`). A `BLOB` keeps every byte, `INTEGER` is
    `int64` and `REAL` is `float64` (both are "unsupported type").
  - A function error passes through `libc.CString`, so it is cut at a NUL,
    and is reported as `SQL logic error: <msg> (1)`, or `SQL logic error
    (1)` when the message is SQLite's own (`modernc_error_text`).
  - A `bool` result is integer 1 or 0.
  - A nil `[]byte` binds as `NULL`.
- **Q-109 `AppendURLParams` re-encodes the whole query.** The keys come
  out sorted (`Values.Encode`), and an existing pair that does not parse is
  dropped silently. A Windows path parses as a URL with scheme `c`, and the
  ported Go test pins it.
- **Q-110 Raw bytes in MySQL DSN errors.** Some error texts carry the
  DSN's bytes unchanged, invalid UTF-8 included: an unknown network's default
  address, the TLS config and server public key names, bool values, time
  zones, `timeTruncate`, and a reserved TLS key.

### Config loading: TOML and dotenv (WP-1-15)

- **Q-111 One expression at a time.** `Unmarshal` parses one top-level
  expression, checks its keys and decodes it before it parses the next
  (`unmarshaler.go:251-260` `fromParser`, `unstable/parser.go:99`
  `NextExpression`). An error in expression n (a key defined twice, an
  integer out of range, an impossible date) wins over a syntax error after
  it, even on the same line: `a=1\na=2 x` is `toml: key a is already
  defined`, `a=2021-13-01 x` is `toml: impossible date`, `a=1\n=\na=2` is
  the syntax error. Evidence: golden `edge order 0` to `14`.

- **Q-112 Swapped arguments and a double space** in
  `internal/tracker/seen.go:268`: `toml: key %s already exists as a %s,  but
  should be an array table` gets the kind first and the key second
  (`toml: key value already exists as a fruit,  but should be an array
  table`). Evidence: golden `go-toml toml_testgen_test.go:114`, `:1629`,
  `:1644`.

- **Q-113 Message texts kept verbatim**, grammar included: `toml: key a
  should be a table, not a array table` (`seen.go:213`), `hour cannot be
  greater 23`, `seconds cannot be greater 60` (`decode.go`), `couldn't parse
  octal number: strconv.ParseInt: parsing "1000000000000000000000": value out
  of range` (strconv's text inside). Evidence: golden `edge order 9`, `edge
  integer 63`, `TestUnmarshalDecodeErrors` port.

- **Q-114 `nan`, `+nan` and `-nan` are all `math.NaN()`**, bits
  `0x7FF8000000000001`; the sign of `-nan` is dropped (`decode.go:285`).
  Evidence: golden `edge keyword 18` to `20`, unit test `nan_is_gos`.

- **Q-115 A leading zero at the very end of the document.** The `0`
  prefix check in `scanIntOrFloat` needs more than two bytes
  (`unstable/parser.go:1125`): `a=01` is `toml: leading zero not allowed on
  decimal number` (1:3), `a=01\n` is `toml: expected newline but got U+0031
  '1'` (1:4), `a=0x` is `expected newline but got U+0078 'x'`. Evidence:
  golden `edge integer 6`, `7`, `36`.

- **Q-116 The space of a date-time skips a byte.** `scanDateTime` adds 2
  at a space followed by a digit and the loop adds 1
  (`unstable/parser.go:1083`), so the byte after the first digit of the time
  is never looked at: `a=1979-05-27 0x:32:00` is `toml: expected digit (0-9)`
  at 1:15, not a newline error. Evidence: golden `edge datetime 65`.

- **Q-117 Two messages for a control character in a string.** A string
  without escapes is validated in one pass and reports `invalid UTF-8`
  (`unstable/parser.go:711`, `:904`); once it has an escape, the slow path
  reports `invalid character U+0001` (`:797`, `:957`). Evidence: golden
  `edge basic string 33`/`34`, `edge multiline basic string 36`/`37`.

- **Q-118 Fractional seconds past nine digits are cut, not rounded**,
  and the precision stays 9 (`decode.go:253-262`): `00:00:00.1111111119` is
  111111111 ns. Evidence: `TestIssue710` port, golden.

- **Q-119 `\e` is accepted** (TOML 1.1, `unstable/parser.go:773`,
  `:932`). Evidence: golden `toml-test tests/valid/string/escape-esc.toml`,
  `config golden toml 1.1 escape e`.

- **Q-120 `%c` of a byte** prints the byte as the rune of the same
  number, so byte 0xEF is `ï` (`unstable/parser.go:867`, `:1237`, `:1241`);
  `%#U` quotes the rune only when `strconv.IsPrint` says so (`U+0020 ' '`,
  `U+00A0` unquoted). Evidence: golden (`invalid character at start of key:
  ï` and others from mutations), unit test `sharp_u_matches_go`.

- **Q-121 A value that starts with `}` is an `incomplete number`**:
  `parseVal` hands anything it does not recognise to the number scanner
  (`unstable/parser.go:1198`), so `a={a=}` fails there. Evidence: golden
  `edge nesting 2`, `3`.

- **Q-122 Key redefinition follows go-toml's tracker**
  (`internal/tracker/seen.go`): a table header marks its parents explicit
  (`setExplicitFlag`, `:144`), re-entering an array table forgets the keys of
  the previous element (`clear`, `:101`, `:270`), each inline table has its
  own tracker, and `[foo.bar.baz]`, `[foo]`, `[foo.bar]` is accepted
  (issue 739). Evidence: golden, `TestUnmarshal` and `TestIssue*` ports.

- **Q-123 Where an error at the end of the document is reported
  depends on the input's spare capacity.** Go reslices `b[len(b):]` to zero
  capacity without moving the pointer when `len == cap`, and
  `danger.SubsliceOffset` (`internal/danger/danger.go:11`) then measures
  from the start of the slice being scanned. With `len == cap`, `a=[` is
  reported at 1:3, `a = "abc` at 1:5, `x=1\ny=[1,` at 2:5; with spare
  capacity at 1:4, 1:9 and 2:6. viper always has spare capacity: it reads
  the file into a `bytes.Buffer` (`viper.go:1707`), whose `ReadFrom` leaves
  at least 512 free bytes. The port gives the spare-capacity result
  (D-66); the golden tool decodes through `bytes.Buffer.ReadFrom` as
  viper does and checks the spare capacity.

- **Q-124 Go's stack limit.** Go's goroutine stacks grow to 1 GB; at
  about 828,000 nested arrays go-toml's `parseVal`/`parseValArray`
  recursion (`unstable/parser.go:355`, `:586`) passes it and the process dies
  with `fatal error: stack overflow` (827,734 levels decode, 828,613 crash;
  measured with go1.27.0 on linux/amd64). See X-38.

- **Q-125 No empty final line.** A trailing `\n`, `\r` or `\r\n` gives no
  last empty line on go1.27.0 (C-25). In `strictParse` an empty line is
  skipped, and inside an open quote it cannot close it, so this changes no
  result; it is visible only to gotenv's own scanner test.

- **Q-126 A `\r` at the end of the buffered data ends the line.**
  `splitLines` (gotenv.go) returns `data[:idx]` when the `\r` is the last byte
  it was given, without waiting to see a `\n`; the `\n` then reads as an empty
  line, which a multi-line quoted value keeps: `A="xxxx\r\ny"` gives
  `xxxx\n\ny` when the `\r` lands on the last byte of the scanner's buffer.
  Where that happens: the first read is the probe's 3 bytes (`ab\r\ncd`
  scans as `ab`, ``, `cd`); after the first lines, the partial line is shifted
  to the front and the buffer refilled, so the break hits at line start + 4096
  (then 8192, …, 65536 as the buffer doubles); through x/text, UTF-16 ASCII
  text arrives 2048 decoded bytes per read and UTF-8+BOM text 4096. 96 golden
  cases show it (`boundary/prefix 0 quoted crlf 4096+0` →
  `x×4092 + "\n\ny"`, `…4096-1` and `…4096+1` → one `\n`; also 8192, 16384,
  32768, 65536, `utf16le … 2048+0`, `utf8 … 4096+0`, 24 random files).

- **Q-127 The opening-quote check indexes the wrong string.**
  `idx = strings.LastIndex(strings.TrimSpace(val[1:]), quote); if idx >= 0 &&
  val[idx] != '\\'` (gotenv.go `strictParse`): `idx` is into the trimmed
  `val[1:]` but is used on `val`. With no space after the opening quote it
  tests the byte before the last quote; with spaces it tests some other byte.
  `A=" x\"` closes on its line (`A` = ` x\`), `A="  x\"⏎y"` closes on line 1
  and then fails on `y"`, and `A="\t\""⏎B=1` stays open to "missing quotes".

- **Q-128 A backslash before the last quote of a line keeps the value
  open,** for single quotes too: `A='x\ny\$FOO\\'` → "missing quotes";
  `A='x\'⏎y'` → `x\'⏎y`. Continuation lines test only the last quote of the
  line and the byte before it.

- **Q-129 `StrictParse` rejects every `export NAME` line.** `export B`
  never matches `linePattern` (a separator is required), so `checkFormat` runs
  `parseExport`: the line starts with `export` (`HasPrefix`, so `exporter B`
  counts), it is split at the first space only, and if the rest is not a key
  set earlier in the file (the environment is not consulted) the error is
  "line `…` has an unset variable"; otherwise it is "doesn't match format".
  `export  B` looks up ` B`; `export\tB` has no space and only gets "doesn't
  match format". gotenv's `TestParse` row "allows export line" passes only
  because `Parse` drops the error.

- **Q-130 Double-quoted escapes.** `\n` and `\r` are replaced first, then
  `\X` becomes `X` for any `X` but `$`, then `\$` is left to the expansion. So
  `"\\n"` is a line break, `"\t"` is `t`, `"\\$FOO"` is `$FOO` and `"\é"` is
  `é`. Unquoted values keep their backslashes except before `$`.

- **Q-131 Expansion.** `variablePattern` takes the braces as optional on
  each side, so `${FOO` and `$FOO}` expand; names are `[A-Z0-9_]+` (`$1`,
  `$FOO_` expand, `$foo`, `${a.b}`, `${}` and `$` stay). The process
  environment wins even when the variable is set to "", then keys set earlier
  in the file (`A=1⏎A=${A}2` reads the environment's `A` if set), else "".
  Expanded text is not expanded again.

- **Q-132 A too-long line inside a quoted value is "missing quotes".**
  The continuation loop ends when `Scan` fails and `quote` is still set, so
  `bufio.ErrTooLong` is never reported there. Elsewhere it is
  "bufio.Scanner: token too long". A line must fit with its terminator in
  64 KiB (65535 bytes and `\n` fit; 65536 do not, also at EOF where the
  buffer is full before the reader says EOF); a value of many short lines may
  be longer.

- **Q-133 x/text's decoders.** UTF-16: a lone surrogate is U+FFFD per
  unit, but a trail surrogate followed by another trail surrogate is one
  U+FFFD for both (`isHighSurrogate` tests DC00–DFFF); an odd last byte is
  U+FFFD; a second BOM is kept as U+FEFF, which is not white space, so the
  line fails. UTF-8 with a BOM: each maximal ill-formed subpart is one U+FFFD
  (W3C), unlike Go's one per byte, so a file with a BOM never yields invalid
  UTF-8.

- **Q-134 `\r` alone ends a line,** inside quotes too (`A="x\ry\r"` →
  `x\ny\n`); `\r\n` inside quotes becomes `\n`.

- **Q-135 Separators.** `=` takes white space on both sides; `:` needs at
  least one white space after it and none before (`A:1`, `A :1` fail). The
  multi-line check looks for `=` before `:`, so `A: "x=y` is the unquoted
  value `"x=y`. `A==1` is `=1`. Leading and trailing Unicode spaces of a line
  are trimmed, but `\v` or U+00A0 between the key and `=` fails, while after
  the `=` `strings.TrimSpace` removes them.

- **Q-136 Comments and trailing text.** `#` starts a comment anywhere in
  an unquoted value (`A=x#y` → `x`, `A=#x` → ""); a quoted value followed by
  text on its line is read whole as an unquoted value (`A="x" y` →
  `"x" y`); `A="x" "y"` → `x" "y`; across lines the same input fails.

- **Q-137 Invalid UTF-8 is a non-space rune.** `TrimSpace` stops at an
  invalid byte; the negated classes match it; error texts carry the bytes.

### Neural networks: common/nn (WP-1-11)

- **Q-138 Shape checks.** `add`, `sub`, `mul`, `div` and `pow` require
  the second shape to be a suffix of the first, with Go's messages
  (`functions.rs`). `TestTensor_*` and `nn_go_*` cover the messages.

- **Q-139 Reads past the end.** `embedding` and `slice_indices` read past
  the end of a slice where Go's unchecked index does (`nn.rs` module docs).

- **Q-140 Shared data.** `slice`, `flatten` and `reshape` share their data
  with the source, so writes through one show through the others (`Tensor` docs).

- **Q-141 `Load` panics.** Where Go panics while loading a model, `load`
  returns `Error::Panic` with Go's message (`layers.rs`, around lines 451-474).

- **Q-142 Go's runtime messages** for index and slice bounds (`golang.rs`,
  tested by `nn_go_bounds_messages`).

### Float formatting and case mapping (WP-1-15)

- **Q-143 Shortest float digits settle ties to even.** Go's shortest float
  formatting settles a tie between two equally close shortest forms on the even
  digit; Rust's `{:e}` takes the larger. 2^-25 prints as Go's
  `2.9802322387695312e-08` (Rust alone: `...313`), and the float32 158164.125
  as `158164.12`. `gostrconv::format_float` (`ftoa.rs`) takes Rust's digits and
  settles the tie as Go does. `ftoa_golden.json` holds 75 such ties among 4,077
  floats, every format, precision -1..40 and both bit sizes, and fails on all 75
  without the rule.
- **Q-144 Case mapping is Go's, not Rust's.** viper lower-cases keys with Go's
  `strings.ToLower` and mapstructure matches them to fields with
  `strings.EqualFold`. Both use Go's simple, one-code-point-to-one-code-point
  mappings from Unicode 17.0.0, not Rust's full `to_lowercase` (`İ` becomes `i`,
  a final sigma stays `σ`). `gounicode` ports `unicode.ToLower`,
  `unicode.SimpleFold`, `strings.ToLower` and `strings.EqualFold` over tables Go
  generates (`contracts/tools/casegolden`). `case_golden.json` checks ToLower on
  every non-ASCII cased code point and about 7,000 strings, and EqualFold on
  folding orbits, mixed strings and near misses, both ways round.

### Config loader (WP-1-15, config.go)

- **Q-145 Duplicate keys that differ only by case.** `config.toml` with
  `Port = 1111`, `PORT = 2222` and `port = 3333` under `[master]`. Go's viper
  walks its key map in random order, so the golden file records two outcomes:
  `master.port` 1111 or 2222. The port gives 2222, the second recorded outcome,
  because `viper::insensitivise` keeps the last spelling other than the
  lower-case one (`insensitivise_keeps_the_last_other_spelling`). The port's
  choice is pinned by `pinned_alternatives`, which asserts it exactly.
- **Q-146 A file value shadowing an env-bound key.** `master = 5` in the file
  and `GORSE_MASTER_PORT=4` in the environment. Go's `AllKeys` order is random,
  and viper's `deepSearch` replaces an intermediate scalar with a map. If
  `master` is walked after `master.port`, the scalar wins and the error is
  `decoding failed ... 'master' expected a map or struct, got "int64"`. If it is
  walked first, the nested map wins and Validate reports
  `n_jobs must be greater than 0`. The port walks keys in sorted order, so the
  nested map wins and the result is the second recorded outcome. Pinned by
  `pinned_alternatives`. The rule that a scalar on the path becomes a map is
  how `viper.rs` models viper's `deepSearch`. The golden file shows both
  outcomes, which is the evidence that the order matters.
- **Keys are lower-cased** the way viper lower-cases them (Q-144, Go's case
  mapping).
- **`json.Marshal` of a NaN or infinite float** fails with
  `json: unsupported value: NaN` (or `+Inf`, `-Inf`). The port returns the same
  text from `marshal_json` (golden `meta_error`).
- **Go's `encodeFloat`** writes `e-09` as `e-9` and uses exponent form only for
  magnitudes below 1e-6 or at least 1e21.
- **Go's JSON string escaping** (`<`, `>`, `&` and the control characters) is
  reused from `gorse_common::expression::go_json_quote`.
- **Durations** are integer nanoseconds in the JSON form and Go duration strings
  in TOML.

### YAML decoder (WP-1-15, yaml.v3 port)

- **Q-147**: an unmarshal error for a `!!map` or `!!seq` tag includes the raw
  value text, as yaml.v3's `terror` does (`decoder.rs`). Four golden error
  texts depend on it.

- **Q-148**: a latched scanner error is reported on parser entry and again
  after the state machine, which is yaml.v3's `peek` behaviour (`parser.rs`).

- **Q-149**: a reader error is raised only when the scanner needs characters
  past the decoded window. Decoding keeps the characters before that window
  (`reader.rs`, `scanner.rs` `ensure`).

- **Q-150**: a scanner error message gets one extra line, as yaml.v3's
  `fail()` gives it.

- **Q-151**: comment marks decide where BLOCK-END tokens fall
  (`scanner.rs` `unroll_indent`). Ported as in `scannerc.go`.

### Parallel, rate limits and Go context (WP-1-6)

- **Q-152 A per-minute limit from 1 to 59 stops the master.** The bucket
  holds `rpm/60` tokens, refilled with `rpm/60` a second; for 1 to 59 that
  is 0 and `NewBucketWithQuantum` panics `token bucket capacity is not > 0`
  inside `NewMaster`. The config does not validate `chat_completion_rpm`,
  `chat_completion_tpm`, `embedding_rpm` or `embedding_tpm`. The backoff is
  already set when the requests bucket panics; a `tpm` panic comes after the
  requests limiter is set.

- **Q-153 Limits of zero or less keep the previous limiter**, and a very
  large `rpm` gives a backoff of 0.

- **Q-154 `Split(a, 0)` with a non-empty `a` panics** (integer division by
  zero; Rust's panic text differs). An empty `a` gives no chunks for any `n`.

- **Q-155 `Parallel` reports the lowest failing job, not the first
  failure.** A failing worker stops while the others run the remaining jobs;
  a panicking job (recovered) counts as a success.

- **Q-156 `Detachable` with `nWorkers` 0 blocks until the context is
  done; `Detach` with `nMaxDetached` 0 blocks forever** (Go's unbuffered
  channels).

- **Q-157 juju's int64 arithmetic wraps.** `Take(math.MaxInt64)` while
  tokens are owed wraps the balance positive and succeeds at once; a wait
  whose tick product overflows comes out negative (572 of the golden's
  8440 calls).

- **Q-158 juju's clock going backwards** gives negative ticks, which
  remove tokens from a bucket below capacity.

- **Q-159 juju's `NewBucketWithRate` test never runs**: it is a gocheck
  suite method without the `Test` prefix. It is ported as a real test and
  passes; the golden also covers its 142857 rates.

### Common utilities (WP-1-8)

- **Q-160 `sizeof` charges unused slice capacity at 24 bytes per slot.**
  Go's `sizeof` asks for `v.Type().Size()` of the slice type, which is the
  slice header (24 bytes), not the element size, so the unused capacity is
  charged at 24 bytes per slot. Kept on purpose. The expected numbers in the
  `sizeof` tests come from a Go 1.27.0 program run on the same values; that
  program is in the scratch directory and is not committed.

### HTTP mocks and clients (WP-1-12)

- **Q-161 The decision client accepts any 2xx; the reranker client accepts only 200.**
  `common/decision/client.go` checks `200 <= status < 300`. `common/reranker/client.go` checks `!= 200`.

- **Q-162 Error texts are Go's.** Decision: `jev request failed with status N: <body, trimmed>`.
  Reranker: `rerank request failed with status: N, body: <body>`.

- **Q-163 Optional request fields are omitted as in Go.** `top_n` is left out when zero
  (`omitempty`). A criteria with neither outcome encodes as `{}`, and a question without criteria
  leaves the field out.

- **Q-164 The chat mock prefixes replies for model `deepseek-r1` with a `<think>` block**
  (`common/mock/openai.go`, the same branch as Go).

- **Q-165 Stream pieces are cut by bytes, as Go cuts them.** Eight bytes per piece. A piece can
  end inside a multi-byte character. JSON encoding then writes each invalid byte as U+FFFD, which
  `mock::go_string` reproduces.

- **Q-166 Embedding input must be a string.** Go answers 400 with `invalid input type`, and the
  Rust mock does the same.

### Model core (WP-1-16)

- **Q-167 `GetString` panics on any other type.** Go's `val.(string)` panics
  with `interface conversion: interface {} is float64, not string`. The Rust
  `get_string` panics with the same text (it does not log and return the
  default as the other getters do). Test:
  `get_string_panics_on_another_type_as_go_does`.

- **Q-168 The download keeps any body.** `http.Get` does not check the
  status, so an HTTP 404 page is saved as the zip. Here no status check is made.
  Test: `download_saves_the_body_of_an_error_status`.

- **Q-169 A failed download reports the unzip error.**
  `LocateBuiltInDataset` ignores the download's error. The file is left empty,
  and the unzip then fails. Here the same. Test:
  `failed_download_leaves_an_empty_file_and_returns_its_path`.

- **Q-170 Only a missing file downloads.** `os.IsNotExist(err)` on the
  training file is the only trigger. Any other stat error returns the paths with
  no download. Here `fs::metadata` with `ErrorKind::NotFound`.

- **Q-171 The format error.** The message is `format not matchs %v != %v`
  (the typo is Go's), and `%v` prints the `iota` numbers: `format not matchs
  0 != 1`. Test: `locate_rejects_a_format_mismatch_with_go_s_message`.

- **Q-172 Absolute entry names stay inside the destination.** Go's
  `filepath.Join(dst, "/abs/x")` is `dst/abs/x`, so the entry is extracted
  there and the ZipSlip check passes. The Rust `unzip` strips the leading `/`
  before joining. Test: `unzip_writes_an_absolute_entry_name_under_the_destination`.

- **Q-173 `Params` type mismatch log.** Q-16 applies: the log names
  `ParamName`, not the value's type. Reproduced in `type_mismatch`.

- The JSON-float case of Q-15 is reproduced: Reproduced through `ParamValue::Float64` and the
  exact-type `get_int`. Test: `get_int_does_not_accept_an_int64_or_a_float_from_json`.

### Data types (WP-1-17)

- **Q-174 JSON names are the Go field names.** The records have no JSON tags,
  so `ItemId`, `IsHidden`, `FeedbackType` and the rest are written as Go names.
  Embedded `FeedbackKey` and `Item` are flattened. `UpdateAt` is skipped
  (`json:"-"`). Serde's `rename_all = "PascalCase"` gives these names. Go's
  decoder matches names without case; serde does not (see Q-177).

- **Q-175 Map names are written sorted.** `encoding/json` sorts a map's names,
  so `Labels::Object` marshals in sorted order even though it keeps insertion
  order. The golden has an object whose input order differs from its output.

- **Q-176 An error text lists the labels as JSON.**
  `unsupported labels: [1,"2"]` and `unsupported type in labels: bool` are
  returned to REST callers as the 400 body, so they match Go byte for byte.
  The golden covers HTML escaping inside the list (`<`).

- **Q-177 Key matching is case-sensitive, and duplicate names are an error.**
  Go's decoder matches `{"itemid": ...}` to `ItemId`, and takes the last value of
  a repeated name. serde matches exactly and rejects a repeated name. The REST
  decoder (WP-4-x) must handle both, probably with a hand-written `Deserialize`
  on the record types. This WP does not change it.

- **Q-178 REST labels need a `UseNumber` decode.** `Deserialize` gives
  `Float64`, which is right for the database path. The server WP must decode
  request bodies so numbers become `Labels::Number` with their text, as go-restful
  does, since `ValidateLabels` rejects `float64`. `gojson` and serde do not keep
  number text (`serde_json` would need `arbitrary_precision`).

- **Q-179 Nesting depth.** Go's `encoding/json` allows 10,000 levels of
  nesting; serde_json stops at 128. Deep labels from a request or a stored row
  fail to decode in Rust.

- **Q-180 Go's map order in `ValidateLabels`.** Go checks map entries in
  random order, so with several invalid entries Go's error text varies between
  runs. The port reports the first invalid entry in insertion order. The golden
  has at most one invalid entry per map.

- **Q-181 Go's native `int` labels.** Go code can put an `int` into labels,
  and `ValidateLabels` then says `unsupported type in labels: int`. JSON never
  produces an `int`, so `Labels` has no integer variant. Such values are not
  reachable from the wire or the database.

- **Q-182 `mapstructure` tags are not ported.** The record types carry
  `mapstructure` tags. The only non-test `mapstructure` decoder in Go that
  touches the data types is in `master/rest.go`, and it decodes config. No
  Rust code needs the tags yet. The stores (WP-2-x) decide if they need them.

### Cache core (WP-2-4)

- **Q-183 `sort.Slice` is not stable.** Score documents with equal scores
  come out in pdqsort's order, not input order. `gosort.rs` reproduces that
  order. The two golden tests (40 and 200 items, with many ties) were generated
  with Go 1.27.0 by `sort.Slice` on the same inputs and match exactly.

- **Q-184 `Open` chooses among matching prefixes by map order in Go.** The
  port takes the longest matching prefix. Two creators with the same prefix
  replace each other, as `Register` does in Go.

- **Q-185 `Time` values are written and read as Go does.** The writer is
  `Time::string()` (with the `m=+` suffix for times from `now()`, Q-1). The
  golden strings in `cache/mod.rs` tests come from Go 1.27.0's `Time.String()`:
  `2026-01-02 03:04:05.678901 +0000 UTC`, `2023-01-01 00:00:00.0000005 +0000
  UTC`, and the `CEST` case, which reads back as UTC.

### Vector store core (WP-2-8)

- **Q-186 Empty requests skip the collection check.** `AddVectors` with no
  vectors and `GetVectors` with no ids return success for a collection that
  does not exist. `QueryVectors` with `topK <= 0` returns an empty result
  before the check too.

- **Q-187 Query and get timestamps differ.** `GetVectors` returns
  `time.UnixMilli(ms).UTC()`, and `QueryVectors` returns `time.UnixMilli(ms)`
  in the local zone. Both are reproduced.

- **Q-188 `DeleteCollection` restores on failure.** If xvec's `Destroy`
  fails, Go stores the collection back in the map. The port does the same.

- **Q-189 `Close` is idempotent**, and every call after it fails with
  `xvec database is closed`, except a second `Close`, which succeeds.

- **Q-190 `DeleteVectors` removes timestamps strictly before the cutoff.**
  A vector written exactly at the cutoff stays.

- **Q-191 `GetVectors` returns vectors in the order of the ids**, each id
  once, and skips missing ids (`orderVectors`).

- **Q-192 `NoDatabase.Optimize` succeeds.** Every other `NoDatabase` method
  returns `storage.ErrNoDatabase`.

### Blob stores (WP-2-10)

- **Q-193 one timestamp per chunk.** Go stamps every chunk with its own time,
  so the server rejects a second chunk of one upload (`inconsistent timestamp`).
  The port keeps this: the blob is not created and `Close` returns Ok. Test
  `quirk_second_chunk_is_rejected`. A temporary Go probe, since deleted, showed the
  same result.

- **Q-194 4 MiB chunks exceed the gRPC limit.** A 4 MiB chunk plus its framing
  is larger than gRPC's default 4 MiB receive limit. The server refuses it
  (`received message larger than max (4194332 vs. 4194304)`) and no blob is created.
  A 3 MiB write is accepted. Test `quirk_full_size_chunk_exceeds_grpc_limit`.

### MongoDB cache store (WP-2-7)

- **Q-195 An empty write of time series points is an error.** `AddTimeSeriesPoints`
  with no points returns "must provide at least one element in input slice" (the
  driver's `ErrEmptySlice`). The other writes return nil on an empty list.
  Observed with the Go oracle on 2026-10-28.

- **Q-196 `Purge` leaves the time series points.** Go purges the values and the
  documents collections only. Reproduced.

- **Q-197 Scan loops swallow cursor errors.** `Scan`, `SearchScores` and
  `ScanScores` stop at a cursor error without returning it, because Go never
  calls `cursor.Err()` there. `GetTimeSeriesPoints` does check it, so the port
  returns that error. Reproduced.

- **Q-198 Times are stored in milliseconds, with Go's formula.** Go writes
  `t.Unix()*1e3 + t.Nanosecond()/1e6`. Oracle: `time.Unix(-2, 999999999)` is
  stored as -1001 ms and read back as `1969-12-31 23:59:58.999 +0000 UTC`.
  Whole-nanosecond truncation would give 23:59:59.000, so the port uses Go's
  formula (`to_datetime`). Positive times behave the same either way.

- **Q-199 `IsHidden` is always false when read.** Go's BSON decoder reads the
  key `ishidden`, which the store never writes. The search filters hidden
  documents, so reading `is_hidden` gives the same result, and the port sets
  false directly. No observable difference.

- **Q-200 Go sends one `$set` per patched field.** `UpdateScores` with two or
  more fields emits duplicate `$set` keys. The server applies them all: the
  oracle's patch of `is_hidden`, `score` and `categories` changed all three. The
  port sends one `$set` with the same fields, so the result is the same.

- **Q-201 A negative `begin` is refused by the server.** The driver's skip is
  unsigned, so the port refuses it itself, with the server's text:
  `(Location51024) BSON field 'skip' value must be >= 0, actual value '-1'`
  (oracle, MongoDB 6.0.28). `limit` of 0 is unlimited on the server, as in Go;
  the oracle confirmed it.

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

- **X-9 `common/floats` beyond the release builds' behaviour** (`PORTING.md`
  D-23, D-25). Go's SIMD builds index `a[0]` before calling a kernel, so the
  kernel operations panic on empty slices on amd64 and arm64; the port returns
  what the scalar loops return (nothing written, `0` for `Dot` and
  `Euclidean`). `floats_golden.json` records Go's panics and the test checks they
  are exactly those. Go's kernels read and write outside the slices when `MM`'s
  dimensions exceed them; the port asserts the extents first, and with `k = 0`
  computes the kernels' result (`a·bᵀ` stores zeros, the rest leave `c` alone)
  without handing them pointers into empty slices. A `Feature` naming features
  the CPU lacks (an illegal instruction in Go) uses only those the CPU has. Not
  ported, because no release build runs them: the MKL and OpenBLAS backends
  (`mkl`/`openblas` build tags), Accelerate (cgo on macOS arm64; it also
  overwrites `c` where the kernels add, Q-32), the `GOEXPERIMENT=simd` build, and
  the riscv64 RVV, loong64 LASX and SVE2 kernels: on architectures other than
  x86-64 and AArch64 the port runs the scalar loops. `GODEBUG` fields Go warns
  about on stderr are ignored silently. `Feature::NONE.mm` runs its row
  operations with the scalar loops, where Go's internal `Feature(0).mm` would
  use the process's level; Go has no exported way to reach that.

- **X-10 `pop` and `peek` on an empty `PriorityQueue` return `None`** where Go
  panics (`PORTING.md` D-23). Every Go caller checks `Len()` first
  (`common/ann/hnsw.go`, `common/ann/bruteforce.go`) or holds a queue that
  cannot be empty there. Pushing a NaN weight still panics with Go's message,
  as does `reverse` of a queue holding one.
- **X-11 `PriorityQueue::unmarshal` returns a new queue.** Go's `Unmarshal`
  fills an existing queue and on an error leaves it half-written; its only
  caller, `HNSW.Unmarshal`, creates a fresh queue for each call and discards
  everything on an error. A negative element count is an error where Go panics
  in `makeslice` (as X-7).
- **X-12 `TopKFilter::new` takes `usize`.** Go takes an `int`; a negative `k`
  keeps nothing, exactly as `k = 0` (golden `k=-1` cases, replayed with
  `k = 0`). Callers holding a signed value pass `usize::try_from(k).unwrap_or(0)`.
- **X-13 Methods promoted from Go's unexported `_heap` are not ported.** `Less`
  and `Swap` on both types, and `Pop() any` on `TopKFilter` (which removes the
  last array element, not the top), are exported in Go but no Gorse code calls
  them.

- **X-14 The consistent-hash ring has no internal lock.** Go's `Consistent`
  embeds a `sync.RWMutex` (so `Lock`/`RLock` are part of its exported API) and
  locks inside every method. The Rust ring takes `&mut self` to change, and a
  caller that shares one wraps it in a lock. Gorse builds a fresh ring for each
  `pullUsers` call and uses it from one goroutine.
- **X-15 The ring's exported `Host` struct is not a public type.** Loads are
  kept as host name to `i64`; `get_loads` returns the same map Go's `GetLoads`
  does. No Gorse code uses `Host`.
- **X-16 Ring panic texts differ.** Where Go panics with a runtime error
  (`integer divide by zero`, `nil pointer dereference`), Rust panics with its
  own text; the golden tests check that both panic at the same call.
- **X-17 `MaxLoad` above `i64::MAX`.** It converts `ceil(total / hosts * 1.25)`
  to `int64`; above `i64::MAX` (a total load over about 7.4e18) Go gives
  `i64::MIN` on amd64 and saturates on arm64. Rust saturates, as Go on arm64.
- **X-18 Ring keys and hosts are `&str`.** Go strings may hold invalid UTF-8;
  worker uuids are hex and user ids are UTF-8 in every Gorse store (X-8).

- **X-19 `unmarshal_json` of text that is not JSON returns serde_json's
  message**, not Go's scanner text. Go's decoders check the whole document
  before calling `UnmarshalJSON`, so only direct calls can pass such input, and
  none in Gorse does. Valid JSON (including lone `\u` surrogates and invalid
  UTF-8, which Go turns into U+FFFD) is decoded like Go.
- **X-20 serde errors for feedback type expressions carry serde_json's position
  suffix** (` at line L column C` after Go's text). A string element with a lone
  surrogate escape or invalid UTF-8 fails in serde_json before reaching the
  visitor, with serde_json's message, where Go fails with
  `invalid expression format`; both reject it.
- **X-21 `Hnsw::add` takes `&mut self`.** Go's HNSW lets goroutines insert and
  search at once behind its own mutexes, so the graph depends on scheduling.
  Here inserts are serialized by the caller (a `Mutex`, as the ported
  `test_multithread` does) and searches take `&self`; the graph then depends
  only on the insert order and the draws.
- **X-22 `Hnsw::unmarshal` is atomic and never panics on counts.** Go replaces
  fields as it reads, so a failed `Unmarshal` leaves a half-replaced index, and
  it allocates from the counts, panicking with `makeslice: len out of range`
  on a negative or huge one. Rust returns `NegativeCount` for a negative count,
  grows its vectors as the data arrives (a huge count ends at the end of the
  input with an error), and changes nothing on an error (D-23).
- **X-23 An insert into an unmarshalled empty index.** Go deadlocks: the index
  is no longer fresh, so the first vector becomes its own enter point and the
  insert read-locks the vector whose write lock it holds (the golden generator
  detects it from the child process's deadlock report). Rust has no such lock;
  the vector is linked to itself.
- **X-24 Narrower argument types.** `q` and `k` are `usize`: Go's negative `q`
  is an error and a negative `k` returns nothing, and no Gorse caller passes
  either. The distance is a `fn` pointer, where Go takes any function value;
  Gorse passes only top-level functions (`logics/cf.go:32`).
- **X-25 The monotonic clock starts at first use**, not at process start: Go
  takes `startNano` at package init, Rust has none.
  `gotime::init_process_clock()` starts it; nodes call it first thing in
  `main` (WP-4-*). The `m=+` values differ between any two processes anyway.
- **X-26 No build-time GOROOT.** Go's `runtime.GOROOT()` falls back to the GOROOT
  baked into the binary, so a Go node finds `lib/time/zoneinfo.zip` on a
  machine with the toolchain. A Rust binary has none: the zip fallback applies
  only with `$GOROOT` set or after `internal::set_default_goroot`. The system
  zone directories and `$ZONEINFO` work as in Go. Gorse's release images are
  `FROM scratch` with no toolchain and no tzdata (`cmd/*/Dockerfile`), so there
  only UTC and fixed zones load, in Go and in Rust alike.
- **X-27 `time/tzdata` (the embedded zone database) is not ported**: Gorse does
  not import it.
- **X-28 A truncated zip central directory** is reported as `corrupt zip file`;
  Go slices without a length check and panics (`PORTING.md` D-23).
- **X-29 `serde::Deserialize` for `Time`** receives the unescaped string, where
  Go's `UnmarshalJSON` sees the raw JSON bytes. `Time::unmarshal_json` takes
  raw bytes and is exact; serde errors carry serde_json's position suffix.
- **X-30 Errors are `Result`s.** Go returns `(zero Time, err)`; the ported Go
  tests map an error back to `Time::default()` where Go asserts the zero time.
- **X-31 dateparse API shapes:** `parse_in` takes `&Location` (Go's
  `ParseIn(s, nil)` is `parse_any`); `ParserOption` is an enum, not a function
  value; unused Go constants (two date states, two time states, `offsetlen`)
  are left out. Behaviour is the same.
- **X-32 Time panics.** Where Go panics (index out of range, nil parser), the
  port panics at the same inputs with Rust's message; Go's deferred retry while
  panicking is not emulated (the outcome is still a panic). The golden test
  requires a panic exactly where Go panics.
- **X-33 math32 on arm64 is not bit-exact.** There math32's `Exp` and `Log` run
  the Go code (`stubs_arm64.s`), which the port selects, but Go compiles every
  pure-Go function on arm64 with fused multiply-adds and Rust does not fuse.
  Results on arm64 can differ in the last bit. `PORTING.md` D-25 makes
  linux/amd64 the reference; no arm64 machine was available to measure.
- **X-34 Only the math32 functions Gorse calls are ported:** `Abs`, `Copysign`,
  `Cos`, `Exp`, `Floor`, `Frexp`, `Inf`, `IsInf`, `IsNaN`, `Ldexp`, `Log`,
  `Modf`, `NaN`, `Pow`, `Sin`, `Sqrt`, `Tanh` and the constants `Pi`, `Sqrt2`,
  `Ln10`, `MaxFloat32`. `Log1p` and `Log2` follow (O-3).
- **X-35 `Sqrt` on targets other than x86-64 and aarch64** uses the Go code. Go
  has assembly for more architectures; Gorse ships amd64 and arm64.
- **X-36 A TLS client whose chain does not reach the server's CA (WP-1-4).**
  The Go client sends no certificate (none matches the server's CA hints) and
  the Go server reports `client didn't provide a certificate`; a Rust client
  sends its certificate anyway and the server rejects the chain. The verdict,
  rejected by the server, is the same, and that is what the tests compare.
- **X-37 TLS handshake error texts are rustls's, not Go's (WP-1-4,
  `PORTING.md` D-47).** Credential errors at start-up keep Go's texts.

### Config loading: TOML and dotenv (WP-1-15)

- **X-38 No stack limit.** Where Go dies (Q-124), the port
  decodes: its recursion runs on a stack that `stacker` grows on demand
  (D-65). Mirroring Go would mean aborting the process at a depth that
  depends on Go's frame sizes. Test: `toml_deep::deeper_than_go_stack_limit`
  decodes 1,000,000 nested arrays. It needs about 5 GB and 20 s in a debug
  build, so it is `#[ignore]`d with that reason and run on request:
  `cargo test -p gorse-config --release --test toml_deep -- --ignored`
  (1.9 s, 1.8 GB peak; passes).

- **X-39 Floats with more than 800 digits before the point: closed
  (WP-1-15).** Go's `strconv.ParseFloat` falls back to its 800-digit `decimal`
  for a number with more than 800 digits before the point, a non-zero digit
  past the 800th, and a value the fast path cannot round. That `decimal`
  puts the point after its 800th stored digit (`internal/strconv/atof.go:110`
  and `:131-133`), so Go's value is off by a power of ten: `9007199254740993`
  + 790 zeros + `1e-791` is `9.007199254740993e+08` in Go. The first
  `ParseFloat` (WP-1-4) rounded the exact value instead. The go1.27 port in
  `gorse_common::gostrconv` (D-52) reproduces Go's fallback, and the four
  golden cases that deviated now match Go, so `toml_golden.rs` no longer
  carries a deviation table and requires all 10,970 cases to match.

- **X-40 Bytes that are not UTF-8 in a value or an error text.** Go
  keeps them in its strings; `decode` returns Rust strings, where each byte
  that does not start a valid sequence becomes one U+FFFD, as Go's
  `for _, r := range s` reads it. `strict_parse` returns Go's exact bytes for
  every golden case. Only files without a BOM can produce such bytes
  (Q-133). Test: `decode_matches_go_except_listed_invalid_utf8` lists the
  31 named cases (`bom/fe fe`, `bom/ff ff`, `bom/fffe after newline`,
  `bom/not a bom fe`, `bom/not a bom ff`, `bom/utf8 partial bom`,
  `bom/utf8 partial bom 1` and 24 `invalid/…` cases) and 582 mutation cases
  by number, checks that exactly those have non-UTF-8 output in Go, and that
  `decode` gives Go's text with the U+FFFD replacement for them and Go's text
  exactly for every other case.

### Neural networks: common/nn (WP-1-11)

- **X-41 `add` of a tensor without elements.** Go loops forever (`i += 0`).
  The port panics with a message saying so (`tensor.rs`, `add`).

- **X-42 Benchmarks not ported.** `tensor_test.go`'s `matMulLegacy`,
  `batchMatMulLegacy` and the `Benchmark*` functions measure speed and assert
  nothing, so they are not ported.

- **X-43 Progress bar.** `TestMNIST`'s progress bar is not ported. The
  per-epoch line and the test accuracy are printed.

- **X-44 Go's top-level `rand` is a stand-in** (D-81): splitmix64
  with a random seed, and Box-Muller for `NormFloat64` rather than Go's ziggurat.
  Same distribution, different values, and Go's values are random too.

- **X-45 Dataset tests ignored.** `TestIris` and `TestMNIST` need files
  from `cdn.gorse.io`, which is blocked (O-1). They are `#[ignore]` as `ann`'s
  are, and read `~/.gorse/dataset/{iris,mnist}`. `TestMNIST` also skips in Go
  unless on Apple silicon or AVX-512. The Rust test does not repeat that check.

### Config loader (WP-1-15, config.go)

- **X-46 `NewTracerProvider` is not ported.** It builds the OpenTelemetry
  exporter from `TracingConfig`. No work package in `porting/work-packages.md`
  owns tracing. See O-17, and its decision there.

- **X-47 YAML cases were ignored, now un-ignored: closed.** `config::tests::test_unmarshal_yaml`
  and the golden `load_yaml_cases` ran as ignored until the YAML decoder merged
  (WP-1-15, YAML part). They pass now.

- **X-48 Log output: closed.** Until WP-1-8 merged, `LoadConfig` had a placeholder logger (D-84). The log-swap merge (0dfc872) replaces it with `tracing::info!` and `tracing::warn!` carrying the same text, and removes `log.rs`.

- **X-49 (closed 2026-10-28) `contracts/config/config.schema.json` and `config.keys.md` are not
  generated** by this port. They come from `contracts/tools/configschema`
  (`jsonschema` reflection and the walk over `GetDefaultConfig`). WP-1-15's
  acceptance names only `meta-config.*.json` and `config.toml`. Closed: the
  integrator ran `go run ./configschema -out <tmp> -src ../../config` in
  `contracts/tools` (go1.27.0). Its four outputs (`config.schema.json`,
  `config.keys.md`, `meta-config.default.json`, `meta-config.sample.json`) are
  byte-identical to the frozen files, so the frozen copies are current. The Rust
  port has no generator and needs none: the master serves the frozen schema
  (`porting/crate-selection.md`, the invopop row), and that WP checks it when it
  lands.

### YAML decoder (WP-1-15, yaml.v3 port)

- **X-50** (non-UTF-8 bytes): a Go string is bytes. Only a `!!binary`
  scalar, or a percent-escape in a tag, can make it non-UTF-8. Rust `String`
  cannot hold those bytes, so each run of invalid bytes becomes one U+FFFD
  (`lossy_runs`). Tags use `from_utf8_lossy`. In the golden, 33 cases differed
  only in their `hex` and `k_hex` fields, which give the raw bytes. The replay
  therefore compares the lossy text and does not compare those two fields.
  Checked by stripping `hex` from the Go outcome and comparing the rest.

### Parallel, rate limits and Go context (WP-1-6)

- **X-51 `gocontext` is partial (WP-1-6).** Deadlines and timeouts, causes, `AfterFunc` and `WithoutCancel` are not ported. Gorse uses `WithoutCancel` for `event.Emit` (`master/tasks.go:126`, `server/rest.go:178`); the work packages that port those callers add it. Values (`WithValue`, used by monitor spans) came with WP-1-8.

### Common utilities (WP-1-8)

- **X-52 Log format and the loggers not ported.** Log lines are tracing's
  JSON, not zap's JSON: the field names and the time format differ. Events go
  to stderr as JSON at info level when no set-up has run, as zap's production
  logger does. Not ported: `OptunaLogger`, the OpenTelemetry handler, and
  `ResponseLogger`. Lumberjack removes old backups in a goroutine after a
  rotation; here the removal finishes before the rotation returns.

- **X-53 `json` NaN and infinity.** Go's `json.Marshal` fails with
  `UnsupportedValueError` on NaN and infinities. `serde_json` writes `null`
  before the formatter sees them, so `marshal` returns `null` where Go errors.
  The test for a failing encoder uses a custom `Serialize` type instead.

- **X-54 `copier` aliasing and errors.** Go's copy can leave the copied
  slice sharing its backing array with a variable the caller still holds. Rust
  ownership cannot express that, so the port copies. Tests check the reuse of
  the destination's buffer through `as_ptr`. Three Go errors (a destination
  that is not a pointer, a type mismatch, a struct name mismatch) are compile
  errors here, so `copy` returns nothing. See the module doc.

- **X-55 Home directory.** Go takes the home directory from
  `user.Current()` (the password database) and stops the process if that
  fails. The port reads `$HOME` and uses an empty path when it is unset.

- **X-56 `datautil` details.** A short CSV row (fewer than five fields) is
  an `Error::ShortRow`; Go panics on the `row[4]` index. Unzipped files get default
  permissions; Go uses `f.Mode()`. The ZipSlip check (`file_path` must be a
  strict descendant of the destination, as Go's `HasPrefix` check requires)
  is ported and tested.

- **X-57 Monitor and `sync.Map`.** Each span guards its fields with a
  mutex and the maps with mutexes; Go uses `sync.Map` and unlocked updates.
  The same sequence of calls gives the same results, and there are no data
  races.

### HTTP mocks and clients (WP-1-12)

- **X-58 The OpenAI mock listens on 127.0.0.1, not on every interface.** Go's
  `net.Listen("tcp", "")` listens on all interfaces. Go's reranker mock already binds `127.0.0.1:0`,
  and the Rust reranker mock matches it. Tests connect only to loopback, so the Rust mock keeps
  itself off the network.

- **X-59 The mock's JSON leaves out two fields.** Go writes `content_filter_results` and the
  unnamed `MultiContent` field on chat messages. The Rust mock writes neither. No Gorse code reads
  either.

- **X-60 `start()` returns `Ok(())` after `close()`.** Go returns `http.ErrServerClosed`.

- **X-61 Chunked request bodies get a 400.** Go's server reads them. The Gorse clients send
  `Content-Length` bodies, so no test reaches this path.

- **X-62 `"answers": null` is a decode error.** Go's nil map accepts it. Serde rejects `null` for
  a `BTreeMap`. No Gorse caller sends null.

- **X-63 Float32 embeddings are written as `139.0`.** serde_json writes an `f32` with a fraction;
  Go writes `139`. The values are equal, and the Go test compares parsed float32s.

- **X-64 An empty `messages` list gets a 500.** Go indexes `Messages[0]` without a check, and
  net/http then drops the connection. The Rust mock answers 500 with Go's index-panic text, so the
  failure is visible.

### Model core (WP-1-16)

- **X-65 `SuggestParams` is not in the `Model` trait.** Go's
  `SuggestParams(trial goptuna.Trial) Params` needs goptuna's `Trial`, which is
  part of the tuning port (WP-3-3, `model/cf/optimize.go` and
  `model/ctr/optimize.go`). WP-3-3 adds it to the trait, and the models implement
  it there. The other `Model` methods are here.

- **X-66 `init` side effect.** Go's `init` creates `~/.gorse/dataset` and
  `~/.gorse/temp` when any binary starts, and stops the process if that fails.
  The Rust library creates nothing on load. `create_built_in_dirs` does the
  same work and returns the error. The binaries (WP-4-7) call it at startup.
  `locate_built_in_dataset` makes the directories it writes into, as Go's
  download path does.

- **X-67 File permissions on unzip.** Unzipped files get default
  permissions, not `f.Mode()`. Same as X-56 for `datautil`.

- **X-68 `GetRandomGenerator` before `SetParams`.** Go returns a zero
  generator that panics on first use. Here the getter panics. No caller
  reaches it before `SetParams`, since every model calls `SetParams` in its
  constructor (WP-3-*).

- **X-69 `Copy` does not share slices.** Go's `Copy` is shallow, so an
  `IntSlice` value is shared between the copies. `GetIntSlice` has no caller
  in the Go tree (only its definition), so the sharing is not reachable.

- **X-70 `unzip` returns only the error.** Go returns the names it wrote
  before an error, together with the error. No caller reads those names, so
  `unzip` returns the error alone.

### Cache core (WP-2-4)

- **X-71 Strings are `String`, not bytes.** D-56 treats Go strings as bytes
  where they can hold any byte. Here every backend but Redis stores text as
  UTF-8, and Go's JSON and BSON encoders cannot store other bytes, so a
  `String` API matches every reachable input. A Redis value that is not UTF-8
  will be an error in WP-2-5 where Go returns its bytes; WP-2-5 must log that.

- **X-72 A test deadline is a cancel.** `gocontext` has no deadlines. The
  suite's `TestScanScores` timeout case cancels a context from a timer thread
  after 1 ms and accepts `Canceled` (Go accepts `DeadlineExceeded`). The
  backends must check `ctx.err()` between documents for the case to hold.

- **X-73 `Integer` returns only an error.** Go's `ReturnValue.Integer`
  returns the parsed number next to a range error. Every Gorse caller ignores
  the number on error, so `integer()` returns `Err` alone.

### Vector store core (WP-2-8)

- **X-74 Float16 bytes are copied.** Go's `Float16Bytes` and
  `Float16Values` reinterpret memory, so the two slices share storage and the
  conversions allocate nothing. The Rust functions copy and encode little-endian
  explicitly. `TestHalfBytes` keeps its value assertions (the hex string and the
  round trip). It drops the aliasing assertions (`values[0] = ...` changes
  `encoded`, `decoded[1] = ...` changes `values`) and the `AllocsPerRun` checks,
  which test Go's memory sharing, not the store's behaviour. Gorse only writes
  these bytes once, so nothing depends on the sharing.

- **X-75 Exact search, not HNSW.** Go's `xvec` answers queries with an
  approximate index. This port answers them exactly, so the Rust results are
  the true top `topK`. The results equal Go's where HNSW's recall is 1, and they
  differ by its missed neighbours otherwise. The ranking tolerance belongs to
  Phase 3 (metric parity); the open question is O-30. Cost: a query is O(n·d) over the collection, not
  sublinear. `NumJobs` and `SkipUnindexedSegments` are accepted and ignored,
  since exact search has no job count and no index to skip.
  Scores are xvec's, read from `internal/core/algorithm/distance_batch.go` and
  `internal/ailego/math/distance.go` at its pinned version:
  cosine `-(1 - cos)`, Euclidean `-(squared L2)`, dot the inner product. Ties
  go to the smaller id, which xvec leaves unspecified. A sparse query's dot
  product sums the query's overlapping indices in the vector's index order,
  which can round differently from xvec's merge.

- **X-76 The xvec format is Rust's own (refines X-1).** X-1 says a master
  switched between Go and Rust on one `xvec://` directory rebuilds its
  collections on its first training round. This port makes that true like so:
  `Init` skips a collection directory whose `gorse.vectors` is missing or lacks
  the magic, and logs a warning. Go's xvec directories therefore do not load.
  `AddCollection` on a name whose directory holds no log of this format removes
  that directory and creates the collection. The removal is preceded by a
  warning that names the directory. Such a directory is derived data
  the master rebuilds, but the removal is not reversible. A directory with a log
  of this format and no open collection is an error.

- **X-77 Collection names are checked.** The empty name, `.`, `..`, and
  names containing `/` or `\` are rejected with `invalid collection name`. Go
  joins the name into the path, so `a/b` creates a nested directory that
  `Init` never reads again (it lists one level), and `..` escapes the store
  root. Gorse builds names from the config's item-to-item and user-to-user
  names, so this follows X-2 in spirit: names are checked, not trusted.

- **X-78 The collection schema is fixed at creation.** `DescribeCollection`
  reports `QuantizationNone` and `Bits` 0 always, as Go does. The store does
  not keep `Bits`, and it rejects any quantization other than none with Go's
  text `quantization type sq for xvec not supported`.

### Blob stores (WP-2-10)

- **X-79 S3 close returns the upload error.** Go's `S3.Create` runs
  `PutObject` on a goroutine behind an `io.Pipe`. Its error is logged and the
  pipe's `Close` returns nil. The port returns the upload error from `close` and
  logs it. The caller can see the failure. GCS already returns its writer's error
  in Go, and the port matches that.

- **X-80 list strips the prefix the same way.** Go's S3 `List` slices
  `Key[len(prefix):]` and drops one leading `/`. The port does the same with
  `strip_prefix`. The prefix is trimmed of slashes first, as the `Path` join is.

- **X-81 temp file removed on failure.** A failed master upload deletes its
  temp file. Go leaves an `upload-*` file behind (its `os.Rename` runs only after a
  full read).

- **X-82 write errors are returned.** POSIX and master writes return their
  I/O errors. Go's `io.Pipe` writes block or fail the copy goroutine instead.

- **X-83 EOF on the next read.** Go's minio reader returns `io.EOF` with the
  last bytes. The Rust readers report the end on the following read. The tests
  assert that second read returns 0.

- **X-84 missing env panics.** The cloud tests panic when a required variable
  is missing (`required_env`). Go's tests skip. A silent skip would look like a pass.

- **X-85 buckets are created out of band.** object_store has no
  `CreateBucket`, so `gorse-test` (and the Azure container) must exist before the
  cloud tests run. Go creates them in the test.

- **X-86 constructors return a Result.** `MasterStoreServer::new` returns
  `io::Result` where Go calls `log.Logger().Fatal`. The caller decides.

- **X-87 cancelled cloud multipart.** A cancelled write leaves an unfinished
  multipart upload, which the provider's lifecycle rule must clear. A cancelled
  POSIX write leaves the partial file, as Go does when its copy fails.

- **X-88 ctx on every method.** `Store` and `BlobWriter` take `ctx`, as D-122
  requires. Go's blob API has no context.

### Data store core (WP-2-1)

- **X-89 Go's `NewScanOptions` skips nil options.** A `ScanOption` cannot be nil in Rust, so there is nothing to skip. Behaviour is the same for every value that can be built.

- **X-90 The shared suite's faker labels are fixed strings.** Go's `faker` colour and genre names are replaced with fixed strings. The cases only check that a label round-trips, so the values do not matter.

- **X-91 Go's `[]string` and `[]any` labels are one `Labels::Array`.** Go's conversions to `[]any` in the suite (`feedback[0].Labels = []any{"positive"}`, `[]any{"a", "b", "c"}`) are kept as the same value. The distinction is erased by `Labels`, which is the decision of D-117.

- **X-92 The suite's type switches are a `Backend` trait.** Go's `suite.Database.(*SQLDatabase)` and `*MongoDB` checks become `Backend::analyze_tables` (Postgres runs `ANALYZE`) and `Backend::time_stamps` (Postgres rounds to a microsecond, SQLite is exact, MongoDB truncates to a millisecond, others skip). TestTimezone skips for `TimeStamps::Unknown`, as Go's `Skipf` does.

- **X-93 Go's `TestSortFeedbacks` and `TestValidateLabels` are in `types.rs`.** WP-1-17 ported them, so the suite file does not repeat them.

- **X-94 Go's `time.Sleep(time.Millisecond)` is `std::thread::sleep`** in the suite. The suite runs on the test's own thread.

### MongoDB cache store (WP-2-7)

- **X-95 `connect=direct` is read as `directConnection=true`.** Go's
  connection-string parser takes the legacy `connect` option. The Rust driver
  rejects it ("connect is an invalid option"). The suite's URI and the Gorse
  configs carry it. `connect=automatic` is the default, so it is dropped. Other
  options pass through unchanged. `driver_uri` does this.

- **X-96 A nil slice of categories is stored as an empty array.** Go writes a
  nil `Categories` as BSON null. `Score.categories` is a `Vec`, which has no nil,
  so the port writes `[]`. Both read back as no categories. Go's null value is
  still read as no categories.

- **X-97 The context is checked on entry and between documents.** Per D-122:
  `ctx.err()` before each call, and before each document in `scan_scores`. Go's
  driver also cancels an operation under way. Here an operation under way runs
  until the server answers or the driver times out. Dropping the future cancels
  it.

- **X-98 No OpenTelemetry command monitor.** Go sets
  `otelmongo.NewMonitor()` on the client. The port has no command monitor. Spans
  belong with the tracing work (O-17), in the server and master WPs.

- **X-99 Each write is its own round trip.** Same results, more round trips
  (D-155).

## Open questions

- **O-1** The Go CI's model tests read the datasets from `https://cdn.gorse.io`,
  which this environment's network policy blocks. Phase 3 metric parity needs
  them (`PORTING.md` status board).
- **O-2 prost leaves `-0.0` out of proto3 `double` and `float` fields;
  protobuf-go writes it** (its bits are not zero). A Rust sender therefore
  delivers `+0` where a Go sender delivers `-0`, for every such field in
  `gorse-protocol` (prost 0.14.4 encodes `{test, None, -0}` as `0a0474657374`).
  `expression_to_pb_matches_go` asserts the exact difference on 15 cases so a
  prost change is noticed. To be settled with the gRPC services (Phase 2/3):
  a score of `-0` can arise (`logics/cf.go` negates a dot product).
- **O-3 math32's `Log1p` and `Log2`** (`model/cf` and `model/ctr`) are
  `float32(math.Log1p(float64(x)))` and `float32(math.Log2(float64(x)))`, so
  they need Go's float64 `math.Log` (assembly on amd64) and `math.Log1p`
  ported. `common/nn` needs Go's float64 `math.Exp` (assembly with an FMA path)
  for the same reason. They come with the nn part of WP-1-11.
- **O-4 `common/expression` has its own `ParseFloat`** for the strings its
  pattern admits (`crates/gorse-common/src/expression.rs`); it could call
  `gostrconv::parse_float` once WP-1-14's conversion lands (`PORTING.md` D-52).
- **O-5 `rustls`'s own unit tests do not build from the published package**
  (its test data is left out); the patch in `third_party/rustls` is covered by
  `util_tls_server_ignores_a_host_and_port_server_name` and the interop check
  instead (`third_party/rustls/GORSE.md`).
- **O-6** The SQLite stores register `json_contains` as deterministic,
  with two arguments, on every connection (Q-25). The glue must:
  - map SQLite values to `json_contains::Arg`, `TEXT` through
    `modernc_text`;
  - return `NULL` for `None` and an integer for the bool;
  - report errors with `modernc_error_text`;
  - bind an absent blob as `NULL`.

  How to surface `Error::Unhashable`, where Go locks the connection, is that
  WP's call: a query error is the natural one.
- **O-7** The MySQL stores call `probe_mysql_isolation_variable_name`
  with their driver, and `apply_sql_pool` on their pool.

- **O-8 Dropping very deep results.** The compiler's drop of a
  `Value` recurses once per level, so a caller that drops a successfully
  decoded document nested tens of thousands of levels deep can overflow a
  2 MiB thread (Go's garbage collector does not recurse). viper's merging
  and mapstructure's decoding recurse too. The viper part of WP-1-15 should
  decide whether `Value` (shared `value.rs`) gets an iterative `Drop`; the
  tests here drop iteratively.
  Closed on 2026-10-28: `Value` now has an iterative `Drop` (D-95, the YAML
  merge). viper's merging and mapstructure's decoding still recurse per level;
  O-20 tracks that.

- **O-9 `gostrconv::parse_float` and Go's 800-digit decimal: closed.**
  The go1.27 port (D-52) reproduces `decimal.set`'s point placement, so the
  deviation of X-39 is gone.

- **O-10 `cargo doc` fails on the base commit: closed.** `src/value.rs`
  linked ``[`Value::kind_name`]``, which does not exist. The link is now
  `Value::kind` (cherry-picked from 09777e7 into the TOML part), so
  `RUSTDOCFLAGS="-D warnings" cargo doc -p gorse-config --no-deps` passes.

- **O-11 Environment values that are not UTF-8.** Go's `os.LookupEnv`
  returns any bytes; the `env` callback returns `Option<String>`, so the
  caller (the `LoadConfig` port) decides what such a variable becomes
  (`std::env::var` reports it as not Unicode). The decoder uses whatever the
  callback returns byte for byte.

- **O-12 Merge points: resolved.** The `Cargo.toml` (regex, stacker, and the
  dev-dependencies hex and serde_json), `Cargo.lock` and `src/formats.rs`
  conflicts were resolved when the dotenv part merged into the integration
  branch. Later parts add their own lines by hand.

- **O-13 `parallel.For` stand-in: closed.** `deps::parallel_for` (see For the
  integrator) stays until WP-1-6 is on the integration branch. WP-1-6 is merged (2026-10-28), so `gorse_common::parallel::for_` exists; closed by the O-13 merge (30e24d9): batch matmul calls `parallel::for_`, and `deps::parallel_for` is deleted.

- **O-14 `GODEBUG` and `util::rand::math`.** `util::rand::math::use_fma`
  reads the CPU directly, without `GODEBUG`. Go's `math.Exp` takes the FMA path
  only when `internal/cpu` allows it, and `GODEBUG=cpu.avx=off` or
  `cpu.fma=off` turns it off. So WP-1-4's `NormFloat64` and `ExpFloat64` differ
  from Go under those `GODEBUG` settings, and `nn::deps::exp` does not.
  Owner: WP-1-4. Fix: make `use_fma` use `Feature::current()`, as `nn` does.

- **O-15 Dataset tests** (X-45) run once `cdn.gorse.io` is reachable,
  and need `iris.data`, `train.libfm` and `test.libfm` under
  `~/.gorse/dataset/`.

- **O-16 `gorse-expr` keeps its own copies of Go's strconv pieces.** `gorse-expr/src/gostr.rs`
  has its own quote, IsPrint, ParseInt and ParseFloat. Now that it depends on
  `gorse-common`, they could call `gostrconv` instead. Not done in the config
  drafts, to keep that change to the bug fix (C-26).

- **O-17 Who owns `NewTracerProvider`?** Decided by the integrator on 2026-10-28. The Go method is `config/config.go:596`, and its call sites are `server/server.go:257` (WP-4-3) and `master/master.go:123` (the master work package after WP-4-3). The exporter is ported with the first of those work packages, into a tracing module of `gorse-config`, and no WP is assigned before then. Still open.

- **O-18**: yaml.v3 compacts its token queue when the backing array is
  full. `scan_comments` looks at `tokens[len-2]`, so in rare cases Go could
  see a different token. Not emulated. No golden case has shown it.

- **O-19**: `is_bom` checks the BOM at the start of the decode window
  (`buffer_start`). yaml.v3's `is_bom` reads `buffer[0..3]` and ignores its
  index argument, so the two agree only when the window starts at the stream
  start. No golden case differs.

- **O-20** (not exercised): the derived `Clone`, `PartialEq` and `Debug` of
  `Value` still recurse once per level, and so do `viper.rs`'s
  `insensitivise_val` and `cast_to_string_map`, which clones values. A
  10000-deep config could overflow there. No test here exercises that, and it
  is not changed here.

- **O-21 `TestLoadIris` is ignored (WP-1-8).** It downloads from `cdn.gorse.io`, which is blocked here, as with O-1. It is ported with the Go test's assertions (150 rows, 4 values per row, 150 targets) and `#[ignore]`d. Offline tests cover the CSV parse and the unzip, including a ZipSlip archive. Run it when the CDN is reachable.

- **O-22 A test flake and a `detachable` race (WP-1-6): closed.** `for_and_detachable_panics_resume_on_the_caller` failed about 1 run in 10 under load: the panic hook took longer than the 1000 jobs of 1 ms, so every job started before the stop flag. The test now holds the other jobs until job 0 has unwound; the bound `< 1000` is unchanged. In `detachable`, a panicked job released its slot before recording the panic, so the scheduler could start one more job first; the panic is now recorded first. The same test also hung when job 0 started late (the other jobs filled both detached slots). Job 0 now detaches first and says so, and the other jobs wait for it. Every assertion is unchanged.

- **O-23 `TestUnzip` ignored.** It downloads `yelp.zip` from
  `cdn.gorse.io`, which this container cannot reach (O-1). Ported with its
  assertions (2 files) as `unzip_downloaded_yelp_archive`.

- **O-24 `TestLocateBuiltInDataset` ignored.** It downloads `ml-1m` from
  `cdn.gorse.io` (O-1). Ported with its path assertions as
  `locate_built_in_dataset_ml_1m`.

- **O-25 `datautil` download has a 30 second timeout (WP-1-8): closed.** Go's `http.Get` has none, and `datautil.rs` calls `reqwest::blocking::get`, which uses reqwest's 30 second default. Ruling: the timeout goes, as D-111 does for the model core. Owner: the util thread. Closed by af6d9caf: the dataset download sets no timeout (`download_from_url` uses a client with `timeout(None)`), and the test `download_from_url_waits_past_the_default_timeout` serves its body after 31 seconds.

- **O-26 pdqsort's heapSort fallback is not reached by any golden case.**
  `gosort` calls `heap_sort` when pdqsort runs out of its partition budget. No
  golden case gets there. A seeded search of 400,000 random slices of 13 to 24
  keys, and eight structured families up to 400 long, produced no hit. So the
  call from pdqsort is not covered: removing it (the slice is then left unsorted
  on that path) passes every test. `heap_sort` itself matches Go on 1000 cases
  through a verbatim copy of Go's code. To close the gap, an adaptive adversary
  (McIlroy's "killer" for pdqsort) would need to build an input that exhausts the
  budget. Only feedback that defeats pdqsort's pivot choices reaches that path,
  which real data is unlikely to do, but the dataset and logics WPs sort large
  slices, so the gap should close before WP-3-x.

- **O-27 (closed by f7ff3189) The cache core copies Go's pdqsort (WP-2-4).** `cache/gosort.rs` is a
  private port of `sort.Slice`'s pdqsort. `gorse-common::gosort` (WP-1-17) already
  ports the same algorithm for `sort.Sort`, so there are two copies. Owner: the
  cache thread. Before WP-2-5 merges, it becomes a closure adapter over
  `gorse_common::gosort`, so Go's pdqsort is in one place. Closed by f7ff3189: `cache::sort_documents` calls `gorse_common::gosort::sort` through an adapter, and the local copy is deleted.

- **O-28 The shared suite has not run against a backend.** No backend
  exists in this WP, and the acceptance command is `cache::no_database`. WP-2-5,
  2-6 and 2-7 must call `cache::database_test::run` with their store. Until
  then the suite is compiled and clippy-checked only.

- **O-29 Benchmarks are not ported.** `benchmark`, `benchmarkAddDocuments`,
  `benchmarkSearchDocuments`, `benchmarkUpdateDocuments`, the prime table and
  `BENCHMARK_DATA_SIZE` are not run by `go test`. Phase 5 performance work is
  the place for them.

- **O-30 HNSW ranking parity.** Go's `xvec` answers with HNSW, which is
  approximate. This port scans exactly (X-75), so its top `topK` can differ
  from Go's where HNSW misses a neighbour. Option: `gorse_common::ann::Hnsw`
  (WP-1-10) as the index. That would bring the rankings closer to Go's, but not
  identical, since the graph build differs. The exact scan costs O(n·d) per
  query. Phase 3 (metric parity) decides whether that gap is acceptable, and
  whether xvec's index must be ported.

- **O-31 (closed by 3966bf6f) GCS write cannot run on fake-gcs-server.** The XML `PUT` to the
  emulator answers 400 `invalid uploadType` on 1.54.0, 1.56.1 and in the latest
  run. `test_gcs` is `#[ignore]`d with that reason. `test_gcs_read_list_remove`
  covers read, list and remove, with the seed uploaded through the JSON API. The
  write needs an emulator that serves the XML upload, or real GCS.
  Integrator ruling (2026-10-28): the ignored test is not coverage of the write path. object_store's GCS put uses the XML API (`src/gcp/client.rs` in object_store 0.14.2), which fake-gcs-server does not serve, so no test runs the write. Before WP-2-10 is closed, the blob thread adds an in-process test that serves the XML PUT and the multipart calls on a local HTTP listener and checks the request line, headers and body that the store sends. The emulator test stays ignored with this reason. Closed by e5b177c (merged as 3966bf6f): `test_gcs_xml_put_sends_one_request` and `test_gcs_xml_multipart_sends_parts_then_complete` serve the XML calls on a local listener and check each request.

- **O-32 fake-gcs-server needs `-external-url` for path-style XML.** Without
  it, `GET /{bucket}?list-type=2` answers 404. This is an emulator setting, and the
  code is unchanged. Run it as `-scheme http -port 5051 -public-host 127.0.0.1:5051
  -external-url http://127.0.0.1:5051 -backend memory`.

- **O-33 live emulators.** RustFS runs as the `rustfs/rustfs:alpha` container
  on port 9000. Azurite runs from the npm `azurite` package on port 10000 with
  `--skipApiVersionCheck`. Buckets and the container are created before the test
  run. The environment variables are `S3_ENDPOINT`, `S3_ACCESS_KEY_ID`,
  `S3_SECRET_ACCESS_KEY`, `AZURE_STORAGE_CONNECTION_STRING` and
  `GCS_EMULATOR_ENDPOINT`. The S3 keys are development-only and are not recorded here.

- **O-34 a missing blob over gRPC is `Unknown`.** Go's server returns the
  `os.Open` error as a plain error, which gRPC reports as code `Unknown`. The master
  client therefore cannot map it to `NotFound`. The integrator needs to choose: a
  status code on the server side, or matching the error text.
  Integrator ruling (2026-10-28): keep Go's wire behaviour. The gRPC error stays code `Unknown`, and the master client does not map it to `NotFound`. This is faithful to Go. Revisit only if a caller needs `NotFound`.

- **O-35 `log` rotation can overwrite a backup within one millisecond (WP-1-8).** Backup
  names carry milliseconds (`gorse-<time>.log`). Two rotations in the same millisecond
  give the same name, and the second rename replaces the first backup. Go's lumberjack
  names its backups the same way, so this is the Go behaviour, not a port bug.
  The test `log::tests::rotation_keeps_the_backups_the_flags_allow` writes 8 chunks
  that rotate 4 times, and it expects 2 backups. It failed once in the full workspace
  run on 2026-10-28 (gate18: `assertion left == right failed: ["gorse-2026-10-28T11-29-14.070.log", "gorse.log"]`,
  left 1, right 2), and it passed alone three times and in the rerun. Owner: the util
  thread. The fix must keep the name format and the assertion unchanged. The test
  makes its rotations apart in time (for example a 2 ms sleep between writes). It must
  not lower the count. Closed by 0842e4d (merged as 42f9f103): the test sleeps 2 ms before each write and keeps the name format and the count of 2.

- **O-36 The suite is compiled and not run.** `data/database_test.rs` is called by no test in this crate. WP-2-2 (SQL) and WP-2-3 (MongoDB) run it against their stores. The acceptance command of this WP, `cargo test -p gorse-storage data::no_database`, runs only the NoDatabase case.

- **O-37 (closed by the WP-2-1 merge) Merged with the integration head 9c89272 (f8b3662).** The cache and vector cores are in integration now. The `Cargo.toml` and `Cargo.lock` overlap is resolved: `async-trait` and `tokio` come from integration, and this WP adds `futures-core` and `futures-util` (0.3.34). The dev-dependency `tokio` already had `macros`, `rt` and `time`.

- **O-38 Crate-selection rows.** `porting/crate-selection.md` needs rows for `async-trait` (0.1.92), `futures-core` and `futures-util` (0.3.34, Apache-2.0 or MIT), and `tokio` (1.54.1). The integrator adds them at fold.

- **O-39 TestTimezone loads `Asia/Tokyo`** through `gotime::load_location`, which reads the system zoneinfo. The case needs the zone file on the test machine.

- **O-40 Shared-suite cases that need a backend are untested here.** Every case in `database_test.rs` is unverified until WP-2-2 or WP-2-3 runs it. The gate for this WP does not run them.

- **O-41 No callback in the data trait takes a `&str`.** The only `Fn` in this WP is `ScanOption`, which takes `&mut ScanOptions`. The cache core's E0597 comes from its `scan` callback, which this WP does not have. Stream methods return `BoxStream` and take no callbacks.

- **O-42 Opening the store in a node. Closed by D-151 (integration).** The
  registry's `Creator` becomes async, as the integrator ruled. `mongodb::creator`
  is that creator: it returns an `OpenFuture` that opens the store with
  `MongoDB::open`. The integrator registers it, in the commit that changes the
  registry. The suite runs it in `creator_opens_the_store`.

- **O-43 The test without `MONGO_URI`.** The test returns early and reports
  `ok`, as Go's `t.Skip` does. The acceptance run sets `MONGO_URI`, so it runs
  every suite case.

- **O-44 MongoDB versions tested.** Only 6.0.28 (the `mongo:6` image, the
  version PORTING.md pins) is run here. The root `docker-compose.yml` has a
  commented `mongo:4.0`, and the Go CI's compatibility job is not run here. The
  driver's minimum server version is not checked.

- **O-45 (closed by f7ff3189) `gosort` copy.** This store does not sort in Rust (MongoDB sorts
  on the server). The duplicate `pdqsort` in `cache/gosort.rs` is WP-2-4's and
  is not changed here (O-27, owned by the cache core).

## WP-2-5 Redis cache store

- **Q-202 `UpdateScores` checks, then writes, with no transaction.** Go's
  `Watch` runs its function on one connection with no `MULTI`/`EXEC`, so the
  watch guards nothing. `EXISTS` and then `HSET` are two commands another client
  can interleave. The port sends the same two commands.

- **Q-203 `ScanScores` fails on a key deleted between `SCAN` and `HGETALL`.**
  Go's `redis.Nil` check, meant to skip such a key, never fires: `HGETALL` on a
  missing key returns an empty map with no error. The empty `timestamp` then
  fails to parse and the scan returns the error. The port does the same.

- **Q-204 A search with `begin == end == 0` has no `LIMIT`.** go-redis sends
  `LIMIT` only when the offset is positive or the limit is positive. A search
  for `[0, 0)` therefore gets RediSearch's default of 10 results. The port keeps
  that condition.

- **Q-205 `Scan` and `Purge` also visit score documents and points.** Both match
  `prefix*`, so they see the `documents:` and `time_series_points:` keys, with
  the prefix stripped. The port does the same.

- **Q-206 A category token that decodes to nothing panics in Go.**
  `decodeCategory` slices `b[1:]` without a length check. The port returns an
  error for that token.

- **Q-207 A missing time series is matched by text.** Go checks for the
  `TSDB: the key does not exist` and `key does not exist` prefixes. The port
  checks for either text anywhere in the error, since the driver's message can
  wrap the server's.

- **X-104 `Close` drops the connection.** The `ConnectionManager` closes with the
  store, which has the same effect as go-redis's `client.Close()`.

- **X-105 Cancellation is checked at call boundaries.** go-redis aborts a command
  in flight when its context is cancelled. The port checks the context when a
  method starts and after each reply of a score scan, as Go's `ScanScores` does.
  A command already sent runs to completion. Open for the WP-4 review.

- **X-106 `CLIENT SETNAME` is sent once, on the first connection.** go-redis names
  every connection it makes. A `ConnectionManager` reconnect does not resend the
  name.

- **X-107 A cluster sends pipelines one command at a time.** redis-rs refuses a
  pipeline whose keys span slots (CROSSSLOT). Go's cluster client splits the
  pipeline per node. The replies are the same. The port makes more round trips.
  Not measured.

- **X-108 Cluster primaries come from one `CLUSTER NODES` reply.** Go's
  `ForEachMaster` walks the client's slot map. The port scans each node whose
  flags include `master`. Run against a six-node Redis 8.10.3
  cluster, see O-46.

- **X-100 Cluster `addr` parameters take the base URL's credentials.** Go's
  `ParseClusterURL` does the same. The port does not decode percent escapes in
  the parameters.

- **X-101 Each test owns a table prefix and purges it, not `FLUSHDB`.** Go's suite
  flushes the whole database, which would make parallel Rust tests clobber each
  other. The assertions are unchanged.

- **X-102 `rediss://` connects through rustls, with the system roots (was X-81
  before the merge with 7f23f5a).** The redis crate's `tokio-rustls-comp`
  feature is on. In redis 1.8.0 it enables `tls-rustls`, so the feature set is
  the one the coordinator named, and the TLS stack is the workspace's rustls.
  The server certificate is checked against the system roots that
  `rustls-native-certs` loads; on Linux `SSL_CERT_FILE` replaces them. Verified
  against `redis:8` with TLS on (a self-signed CA, SAN `localhost`): all seven
  tests pass with the CA in `SSL_CERT_FILE`. With `SSL_CERT_FILE` unset, the six
  server tests fail with `invalid peer certificate: UnknownIssuer`, and the one
  test that needs no server passes. In this sandbox the inherited
  `SSL_CERT_FILE` is the proxy's CA bundle, which does not trust the test CA.

- **X-103 `skip_verify=true` is honoured on `rediss://`, as go-redis does
  (resolved; decided by the coordinator).** The default still verifies. The
  port maps the parameter to redis-rs's `#insecure` URL fragment, which needs
  the `tls-rustls-insecure` feature. Details and the reproduction are in Q-208.

- **Q-208 go-redis's `skip_verify` turns verification off, and the port now does
  too (reproduced).** go-redis's `ParseURL` reads `skip_verify` only when the
  scheme is `rediss://`, and reads it with its own rules: `true` and `1` are on,
  `false`, `0` and an empty value are off, and anything else is the error
  `redis: invalid skip_verify boolean`. `strconv.ParseBool` is not used.
  go-redis's `ParseClusterURL` (`osscluster.go`) never reads `skip_verify`, so a
  `rediss+cluster://` URL verifies whatever its query says; the port does the
  same, since its cluster nodes are built without the query. Reproduced against
  the TLS server with its CA not trusted (`SSL_CERT_FILE` unset): the default
  fails with `UnknownIssuer`, and `?skip_verify=true` runs the whole suite to
  seven of seven. The cluster rule is from the go-redis v9.18.0 source, not run.
  The tests: `skip_verify_is_read_as_go_reads_it`,
  `skip_verify_sets_the_insecure_mode_of_rediss_only` (mutation-checked: the
  mapping removed fails it), and the TLS tests `tls_skip_verify_connects_to_a_self_signed_server`
  (`REDIS_TLS_URI`) and `tls_verifies_the_server_by_default` (`REDIS_TLS_URI` and
  `REDIS_TLS_UNTRUSTED`, which asserts the CA is not trusted).

- **O-46 The Redis acceptance runs pass on Redis 8 and Kvrocks.** The
  shared suite, the search, update and delete paths, and the time series paths
  all run. The environment's Redis 7.0.15 has no modules, so it was not the
  gate. `redis:8` (Redis 8.10.3, with the `search` and `timeseries` modules)
  came from Docker Hub after one HTTP 429 retry. Each run was
  `cargo test -p gorse-storage --features redis cache::redis`, and each passed
  7 of 7:
  - standalone, `REDIS_URI=redis://127.0.0.1:6391/`;
  - a six-node Redis 8.10.3 cluster on the host network, ports 7001 to 7006,
    one replica per primary, with `REDIS_URI=redis+cluster://127.0.0.1:7001`
    and with the six-address `addr=` form the Go CI uses;
  - `apache/kvrocks:nightly`, `REDIS_URI=redis://127.0.0.1:6666/`, which the
    Go CI also runs.
  A mutation check (the escape rule without `-`) made `escape_characters` fail,
  then the file was restored.

- **O-47 The Go CI's Redis jobs are covered for WP-2-5.** The standalone,
  one-address cluster, six-address cluster and Kvrocks jobs in
  `.github/workflows/build_test.yml` all run here (see O-46). The MariaDB job and
  the rest of the Phase 2 exit gate belong to other work packages.

## WP-2-11 meta store

- **X-109 A nil `Params` encodes as `{}`, not `null`.** `Params` is a
  `BTreeMap`, which has no nil. Go writes `"Params":null` for a model with nil
  params (golden: `int_score`, `nil_params`). Both are read back the same by Go.
  The golden test checks this case explicitly (`NIL_PARAMS`), it is not skipped.

- **X-110 `ToJSON` returns `Result`.** Go's `lo.Must1` panics. The
  error cases are a score that `encoding/json` refuses (a NaN) and
  `ParamValue::Other`, which Gorse never stores. Phase 4 callers decide what to
  do with the error.

- **X-111 `FromJSON` matches JSON keys case-sensitively.** Go's
  `json.Unmarshal` matches `"id"` to `ID` and `"type"` to `Type`. serde does not.
  Go's own writer always writes the exact names, so the rows of both
  implementations match.

- **X-112 A type mismatch stops the decode.** Go's `json.Unmarshal` keeps
  decoding the other fields and returns the first type error. The port stops
  at the first error, so a partly-set model can differ from Go's after an error.

- **X-113 Error texts are not Go's.** `encoding/json` texts (syntax, type
  errors) are serde's and gojson's. The `Scan` failure keeps Go's text
  (`Error::UnsupportedScan`). Only the error/no-error outcome is golden-checked.

- **X-114 `Open` connects now; Go's `sql.Open` connects on first use.** An
  unusable file fails at `Open` here, and at `Init` in Go. Both fail the same
  start-up path.

- **X-115 The DSN's query string is not parsed.** The file is the part
  before any `?`. The busy timeout (10 s) and WAL mode are set on the
  connection, as Go's `_pragma` parameters set them. No caller passes another
  parameter (`master.go:287` passes none).

- **X-116 One connection, not a pool.** Calls are serialised, where Go's
  `database/sql` pool can run them in parallel. Results match; the node has
  one meta writer, so nothing waits on the lock in practice.

- **Q-209 `Equal` does not panic on slice parameters.** Go's `maps.Equal`
  compares `any` values with `!=`, which panics when both sides hold the same
  uncomparable type (for example two `[]int`). The port returns a value and
  does not panic.

- **O-48 Resolved by D-166.** The coordinator approved a `ParamValue`
  variant that holds a JSON value (2026-10-28 12:00Z). The change is in
  `crates/gorse-model-core/src/params.rs` (`ParamValue::Json`), with one case
  added to `tests/params_go.rs` for the accessor mismatch.

- **O-49 Phase 3 must give `cf.Score` and `ctr.Score` Go's encoding.**
  `Model<T>` writes its `Score` with `jsonutil::marshal`, which matches Go for
  numbers and strings, and reads it with serde. A struct score needs serde
  renames that match Go's field names (Go has no tags here), and Go's
  encoding of a `float64` field must be checked against the golden data then.

- **O-50 Node JSON for the REST API is not defined here.** Go's `[]meta.Node` in
  `master/rest.go` is the REST shape. `Node` has no serde impl; Phase 4 adds it
  with Go's field names and RFC 3339 time text.

- Evidence (the integrator's fold of the WP's tests): - Ported from Go, unchanged cases: `TestNodes`, `TestKeyValues` (`sqlite_test.rs`, each on a new SQLite file in a temp dir), `TestModel_Equal` (`database_test.rs`). - Golden, from Go 1.27.0: `golden_test.rs`, 7 tests (`round_trip_matches_go` added with D-166). - Mutation checks (sources restored after each): `float32` written as `float64` fails `to_json_matches_go`; the TTL sign dropped fails `test_nodes` and `go_written_rows_are_listed_and_pruned`; an unsorted object encoder fails `round_trip_matches_go`. - No test is ignored. SQLite is embedded, so no service is needed.

## WP-1-18 dataset

- **Q-210 `floats.FromAny` on an empty list.** An empty `[]any` label is
  numeric (`FromAny` returns `([]uint16{}, true)`), so it becomes FP16 bits, not
  an empty ID list. Reproduced by `floats_from_any`.

- **Q-211 Negative `FreqDict` indices panic.** `String`, `Freq` and the lock
  methods index with a negative `int32`, which panics in Go. Rust panics too.

- **Q-212 `Index.Unmarshal` drops repeated names.** It rebuilds with `Add`, so
  a name repeated in the file keeps its first position and the index can hold
  fewer names than its count. This is in `contracts/model-blobs.md`.

- **Q-213 `SampleUserNegatives` caches.** The first call's samples are kept
  whatever `numCandidates` or the excluded split a later call passes.

- **Q-214 `SplitCF` with `numTestUsers <= 0`** selects every user as a test
  user, as `>= CountUsers()` does.

- **Q-215 `GetItems` of a `UnifiedDirectIndex`** logs an empty warning. Ported
  as `tracing::warn!("")`.

- **Q-216 The error text `wrong foramt`** keeps Go's misspelling.

- **Q-217 Ties in `SplitLatest`.** Go's `sort.Slice` is the pdqsort that
  `sort.Sort` runs. Ported as `gosort::sort` over a `less`/`swap` adapter
  (`LatestFirst`), so equal timestamps end where Go puts them.

- **X-117 `FreqDict.ToIndex` copies.** Go's `Index` shares the dictionary's
  map, so writes to the returned index reach the dictionary. Rust returns a copy.
  Nothing in Go relies on that aliasing on the paths this crate serves.

- **X-118 Splits and the built-in test set copy the dictionaries.** Go shares
  `users`, `items`, `userDict` and `itemDict` between a dataset and its splits,
  and `LoadDataFromBuiltIn` shares the training dictionaries with the test set.
  Rust copies them: the same values, at the cost of memory. `loadTrain` adds
  every integer ID from 0 up to the largest one it reads, so the shared dictionary
  already holds those. The test set's `loadTest` adds to the dictionary only a
  name beyond that range, or a name that is not an integer. Go would then add it to
  the training dictionary too, and Rust would not. Whether the ml-1m files name
  such an ID is unknown here (O-1).

- **X-119 `FreqDict` locks are not `sync.Mutex`.** Each index has a flag under
  a `Mutex` with a `Condvar`, so the lock can be taken and released in separate
  calls from any thread, as Go's can. Unlocking an unlocked index panics, as Go's
  fatal error does. A clone gets new, unlocked locks.

- **X-120 Label ID order inside one map.** Go iterates a `map[string]any` in
  random order, so the IDs of label strings under different keys of one map vary
  between Go runs. Rust assigns them in insertion order. Go's order is not
  defined, so Rust's is one valid order.

- **X-121 `strutil.Pool` is dropped.** `processLabels` interns map keys
  through a pool that returns an equal string. Rust keeps the key strings as they
  are. No value differs.

- **X-122 Malformed dataset lines are errors, not panics.** A line with too
  few fields, an empty test line, or a user ID outside the `negatives` table made
  Go index a slice out of range. Rust returns `TooFewFields`, `WrongFormat` or
  `UserOutOfRange`. A test line whose user is not in the training set still
  panics in `add_feedback`, as Go does, because the feedback table is too short.

- **X-123 A negative length in a serialised index is an error.** Go's
  `Unmarshal` calls `make([]string, 0, n)` and panics on a negative `n`.
  `NegativeLength` is returned instead.

- **X-124 `bufio.Scanner`'s 64 KB line limit is not reproduced.** Go's
  loaders fail on a longer line (`ErrTooLong`). Rust's `BufRead::lines` reads it.

- **X-125 Nil receivers are `Option`s.** Go's `*Index` may be nil and then
  has length 0. Rust has no nil index, so `Option<&Index>` with `map_or(0, Index::len)`
  stands in. `UnifiedIndex` nil is `Option::None` too.

- **X-126 Traits and concrete types.** `CFSplit` and `CTRSplit` are Rust
  traits (`CfSplit`, `CtrSplit`). `Dataset` implements `CfSplit`, and
  `split_cf` and `split_latest` return `Dataset`s, which Go returns as the
  interface. `UnifiedIndex` is an enum (`Map(Box<_>)` or `Direct`) rather than an
  interface, so the type switch in `MarshalUnifiedIndex` is a match.

- **X-127 Dataset labels.** Go's `data.User` and `data.Item` hold `any`
  labels, which a dataset fills with label IDs, FP16 vectors and the rest. Rust
  has `DatasetUser` and `DatasetItem` with a `Value` label. Go's `int` and
  `int32` labels become `Float64` in Rust, since the storage `Labels` has no integer.

- **X-128 Names.** Go's `ID` is `Id`, `NotId` is `NOT_ID`, `CFSplit` is
  `CfSplit` and `CTRSplit` is `CtrSplit`, so clippy's acronym lint is quiet.
  Go's `Get*` methods keep the `get_` prefix, as `gorse-model-core` does.

- **X-129 Go `int` parameters.** `numTestUsers` and `shots` are `i64`, which
  keeps the negative values that Go's `int` allows.

- **O-51 `floats.FromAny` is not in `gorse-common`.** Go's `FromAny` takes any
  slice by reflection. Only its `[]any` `float64` case is ported, as the private
  `floats_from_any` in `gorse-dataset`, because `gorse_common::floats` belongs to
  another WP. `worker/pipeline.go`, `logics/user_to_user.go` and
  `logics/item_to_item.go` call `FromAny` too. Their WPs should move the full
  function into `gorse_common::floats` and let this crate use it.

- **O-52 `CtrSplit` has no implementation.** Its implementations are in
  `model/ctr` (WP-3-2). The trait is ported so that WP can use it.

- Tests (the integrator's fold of the WP's notes): - `dataset/dict_test.go`, `index_test.go`, `unified_index_test.go` and `dataset_test.go` are ported in `src/{dict,index,unified_index,dataset}/tests.rs`, case for case. `TestDataset_LoadMovieLens1M` is ignored (O-1: it downloads ml-1m from cdn.gorse.io, which this container cannot reach). - Extra tests, not in Go: `index_unmarshal_drops_repeated_names`, `index_unmarshal_rejects_negative_length`, `unified_index_nil_round_trip`, `unified_index_unknown_type_tag`, `load_train_and_test_read_ncf_files` and `load_test_rejects_a_line_without_parentheses`. They cover the error paths and the file parsers, which only the ignored test reaches in Go.

- Gates on the merge: - `cargo fmt --all --check`: exit 0. - `cargo clippy --workspace --all-targets -- -D warnings`: exit 0. - `cargo test --workspace --no-fail-fast`: exit 0. `gorse_dataset` unit tests: 18 passed, 0 failed, 1 ignored (`dataset_load_movie_lens_1m`, O-1). - `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`: exit 0. - `cd contracts/tools && go vet ./...`: exit 0.

## WP-2-3 MongoDB data store

- **Q-218 `GetFeedback` with neither bound finds nothing.** Go always sets
  the `timestamp` condition, to an empty document when both `beginTime` and
  `endTime` are nil. An empty document matches only a time stamp equal to `{}`.
  Probe against the Go store (MongoDB 6): `GetFeedback(nil, nil)` returned 0
  records, `GetFeedback(nil, ts)` returned 2. The port always sets the
  condition, as Go does.

- **Q-219 `GetItemStream` keeps items strictly after `timeLimit`.** Go uses
  `$gt`, where the SQL store uses `time_stamp >= ?` (`storage/data/sql.go:952`).
  Probe: a limit equal to a stored time excluded that item, an earlier limit
  included it. The port uses `$gt`.

- **Q-220 The three streams ignore the context.** They run on
  `context.Background()` in Go. The port does too: cancelling `ctx` does not stop
  `GetUserStream`, `GetItemStream` or `GetFeedbackStream`.

- **Q-221 Scan loops ignore cursor errors.** Go's loops never check `r.Err()`
  (except `SearchItems`), so a cursor error ends the rows silently. The port
  does the same, and returns the cursor error from `SearchItems` only.

- **Q-222 `Reconcile` rebuilds the text index on every call.** `listIndexes`
  reports a text index with the keys `_fts` and `_ftsx`
  (`contracts/schema/data/mongodb.search.json`). `mongoSearchIndexKeysMatch`
  compares the expected `field: "text"` keys with those, so it never matches:
  Go drops `gorse_search_index` and creates it again on every call. The port
  does the same.

- **Q-223 JSON number labels are stored as numbers, not text.** The Go
  driver's `JSONNumberEncodeValue` writes a `json.Number` as an int64 when its
  text parses as one, and as a double otherwise. Probe: `n: 3` read back as
  `int64`. `Labels::Number` is written the same way (`number_to_bson`).

- **Q-224 Feedback cursor JSON.** The cursor is the base64 of
  `json.Marshal(FeedbackKey)`, which escapes `<`, `>`, `&`, U+2028 and U+2029.
  The port writes it with `go_json_quote`, so the bytes match Go's.

- **Q-225 Cursors accept non-zero trailing bits.** Go's `StdEncoding` decodes
  `QR==` to `A`. The crate's `STANDARD` engine rejects it, so the port uses its
  own engine with `decode_allow_trailing_bits` (`GO_STANDARD`).

- **Q-226 Upserts write whole structs.** `BatchInsertUsers` and
  `BatchInsertItems` `$set` the whole struct, so a nil slice is `null` and a
  nil label is `null`. `BatchInsertFeedback` inserts users and items with
  `$setOnInsert` of the whole struct, so a new item has `categories: null`,
  `labels: null`, `comment: ""` and the zero time stamp.

- **Q-227 Feedback merge.** With `overwrite` false, the upsert uses `$inc`
  on `value`, `$min` on `timestamp`, `$max` on `updated`, and `$set` on `labels`
  and `comment`. The port uses the same operators.

- **Q-228 Field names.** Go's BSON codec lower-cases the field names, so the
  documents use `userid`, `itemid`, `ishidden`, `updateat` and
  `feedbackkey.{feedbacktype,userid,itemid}`. The port writes the same names in
  the same order.

- **X-130 No `bulk_write`.** The Rust driver's `bulk_write` needs MongoDB 8.0,
  and this store runs on 6.0 too (`mongo:6` in `storage/docker-compose.yml`).
  Each write is sent on its own, in order, and the first error stops the batch,
  as Go's ordered bulk write does. Batches cost one round trip per row. This is
  the cache store's choice too (`cache/mongodb.rs`).

- **X-131 `GetItems` and `GetUsers` with `n = 0`.** Go reads `items[n-1]` when
  the page is full, so `n = 0` panics on an empty collection. The port returns an
  empty cursor.

- **X-132 Labels of an array's elements.** Go's `unpack` does not unpack the
  elements of an array. The port unpacks them too. `ValidateLabels` keeps array
  elements to strings and numbers, so Go's output is the same for every label
  Gorse accepts.

- **X-133 Labels that are int32 or int64.** Go keeps the typed value. `Labels`
  has no integer variant, so the port holds it as `Labels::Number` with the same
  JSON text.

- **X-134 Labels of other BSON types.** A date, an ObjectId or a binary in a
  label returns a backend error. Go returns the typed value. Gorse never writes
  these, so only a document from another program can hold one.

- **X-135 `ErrNotFound` text.** Go wraps `storage.ErrNotFound` as
  `item <id> not found`. The port returns the sentinel with `Error::kind()`
  matching `errors.Is`. The message text is not kept, as WP-2-1's `Error` has no
  wrapped-text variant.

- **X-136 Cursor keys match case-sensitively.** Go's `json.Unmarshal` matches
  the `FeedbackKey` keys without regard to case. The port's `serde` matches them
  exactly. Go's own cursors always use the exact names.

- **X-137 `Close` returns `Ok` after shutdown.** The driver's `shutdown` gives
  no error, where Go's `Disconnect` can return one.

- **O-53 MongoDB 8.0 is not tested.** The suite ran on MongoDB 6.0.x, the
  version in `storage/docker-compose.yml` and the schema dumps. The batch writes
  do not use `bulk_write`, so 8.0 should behave the same, but this is not run.

- **O-54 (closed at merge) The trait doc for `get_item_stream` was wrong for both stores.** It
  says the stream skips items stamped after `time_limit`. Go's Mongo keeps them
  (Q-219), and the SQL store skips items stamped before it. The doc in
  `data/mod.rs` now states Go's behaviour for each store.

- **O-55 The `connect` option mapping is copied.** `driver_uri` (the Go
  `connect=direct` and `connect=automatic` options) and the BSON date helpers
  are private copies of the ones in `cache/mongodb.rs`. They can move to one
  shared module once the two stores are merged.

- **O-56 (closed at merge) No built-in registration yet.** `mongodb::register` registers the
  creator for both prefixes, as Go's `init` does. `data::register_builtin`, added at the merge,
  registers it under the `mongodb` feature, as `cache::register_builtin` does.

- Oracle evidence (the thread's run against MongoDB 6.0.28 and the Go TestMongo): - Go `TestMongo` at add21b5 against `mongo:6` (`MONGO_URI=mongodb://root:password@127.0.0.1:27017/`): 13 subtests PASS (`TestBatchGetItems` through `TestUsers`). - Go writes, Rust reads (`gorse_interop_go`): the user, both items (including the nil categories and labels), the feedback with its labels, and the three counts match the Go values. - Rust writes, Go reads (`gorse_interop_rs`): the same records read back in Go with the same values; the counts are 1, 2 and 2.
