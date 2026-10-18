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
