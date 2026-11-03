# WP-5-4 deviation audit (integrator, 2026-11-03)

**Unexplained diffs: 0.**

- Head audited: `116bfab` on `claude/phase-0-contracts-lqc6fr`. The code is that of `1bc5eb0`, which the court's full gate ran; `116bfab` only adds a verdict and this file.
- Gate on `1bc5eb0`, 12 backends up: every stage exits 0, except REDIS_TLS_UNSET, which exits 101 as required. TEST: 1471 passed, 0 failed, 44 ignored. The golden `rest_golden_matches_go` passes with 0 unexplained routes, and no failing test has no row in Table 1 or 2.
- Counts: 45 `#[ignore]` attributes in our code (2 more in the vendored `third_party/mongodb-3.9.1`, excluded). 648 X/Q/C/O entries in `PORTING_NOTES.md` (Table 3). 308 D verdicts in `PORTING.md`, 5 of which name a deviation.
- Ignore reasons that name no open O-n item: **42 of 45** (Table 1, column "rule"). Ignores that no gate stage runs in their working form: **11** (Table 1, column "run by"). Both are recorded as open items (O-94, O-95), not as diffs. The tests and their outputs are not weakened by either.

## Table 1: every `#[ignore]` in our code

Columns: location, test, the reason as written, the gate stage that runs it, the O-n it names and that item's status, whether the reason names an open O-n item (the brief's rule), and the action WP-6-1 takes.

| location | test | reason as written | run by | O-n named (status) | rule met | WP-6-1 |
|---|---|---|---|---|---|---|
| `crates/gorse-storage/src/vectors/qdrant.rs:722,728,734,740,746,752,758,764,770,778` (10) | purge, collections, vectors, half_vectors, get_vectors, sparse, hidden, dot, delete_vectors, quantization | needs a Qdrant server at QDRANT_URI | VECTOR_ACCEPT (`--include-ignored`) | none | no | none (Rust only) |
| `crates/gorse-storage/src/vectors/milvus.rs:927,933,939,945,951,957,963,969,975,983,1009` (11) | purge, collections, vectors, half_vectors, get_vectors, sparse, hidden, dot, delete_vectors, invalid_sparse_collection, quantization | needs a Milvus server at MILVUS_URI | VECTOR_ACCEPT | none | no | none |
| `crates/gorse-storage/src/vectors/weaviate.rs:632,638,644,650,656,669,675,681,689` (9) | purge, collections, vectors, half_vectors, get_vectors, hidden, dot, delete_vectors, quantization | needs a Weaviate server at WEAVIATE_URI | VECTOR_ACCEPT | none | no | none |
| `crates/gorse-storage/src/vectors/weaviate.rs:663` | sparse | Go's TestSparse skips for Weaviate: sparse vectors are not supported | **not run**: VECTOR_ACCEPT passes `--skip vectors::weaviate::tests::sparse` | none | no | none. Gap: the skip has no entry, see O-95 |
| `crates/gorse-storage/src/cache/redis.rs:1408` | tls_skip_verify_connects_to_a_self_signed_server | needs REDIS_TLS_URI, a self-signed TLS server (O-66) | REDIS_TLS_UNSET (unset form only, exits 101 by design). The TLS-set run is in no stage | O-66 (closed) | no | none. Gap: O-95 |
| `crates/gorse-storage/src/cache/redis.rs:1419` | tls_verifies_the_server_by_default | needs REDIS_TLS_URI and REDIS_TLS_UNTRUSTED (O-66) | as above | O-66 (closed) | no | none. Gap: O-95 |
| `crates/gorse-storage/src/blob/gcs.rs:75` | test_gcs | fake-gcs-server refuses the XML upload that object_store uses (O-31, O-62) | **not run** | O-62 (open), O-31 (closed) | yes (O-62) | none. Gap: O-95 |
| `crates/gorse-model-core/src/built_in.rs:320` | unzip_downloaded_yelp_archive | needs cdn.gorse.io, which this container cannot reach (O-1) | **not run** | O-1 (open) | yes | none. Gap: O-95 |
| `crates/gorse-common/src/datautil.rs:192` | load_iris_downloads_the_dataset | needs cdn.gorse.io, which this container cannot reach | **not run** | none (O-1 is open for the CDN) | no | none. Gap: O-95 |
| `crates/gorse-cmd/tests/playground.rs:155` | playground_ml_100k_imports_the_dataset_and_serves | O-1: dataset from cdn.gorse.io | **not run** in this form. Run on this head: it fails because the binary fetches the CDN directly, as O-1 says. The import is covered by `conformance/tests/playground.rs` in TEST, through the CDN stand-in (X-261, D-297) | O-1 (open) | yes | none. The ignore stays |
| `conformance/tests/vector_cross_read.rs:396,411` | rust_writes_go_checks, go_writes_rust_checks | needs Go and the Qdrant, Milvus and Weaviate servers; see the module docs | CROSSREAD (`--ignored`) | none | no | removed: both run Go |
| `crates/gorse-config/tests/toml_deep.rs:216` | deeper_than_go_stack_limit | needs about 5 GB in a debug build; run with --release -- --ignored | **not run** | none (X-38 explains the behaviour) | no | none (Rust only). Gap: O-95 |
| `crates/gorse-common/tests/util_rand_golden.rs:358` | util_rand_godebug_child | run by util_rand_streams_follow_godebug_in_child_processes | run through its parent test in TEST | names the parent test, not an O-n | no | none |
| `crates/gorse-common/src/util/rand/math.rs:391` | godebug_child | run by exp_and_log_follow_godebug_in_child_processes | run through its parent test in TEST | names the parent test, not an O-n | no | none |
| `crates/gorse-common/src/math32/golden_tests.rs:289` | math32_exhaustive | evaluates each function on all 2^32 inputs; run in release (see the module doc) | **not run**. X-34's table is produced by it | none | no | none (Rust only). Gap: O-95 |
| `conformance/tests/o91_probe.rs:600` | o91_probe_master_replies | seeds two clusters, about half an hour: run with --ignored | **not run**. A probe for O-91 (closed by D-299) | none | no | removed: it runs Go |
| `conformance/tests/mixed_cluster.rs:148` | cf_training_release_timing | O-89: timing run, use cargo test --release -- --ignored | **not run** | O-89 (closed by D-292) | no | removed with the file: it runs Go |

Totals: 42 reasons do not meet the brief's rule (31 server-gated vector tests, 2 Redis TLS tests that cite the closed O-66, 2 cross-read tests, 2 godebug children that name their parent, 1 Iris download, 1 toml test, 1 math32 test, 1 probe, 1 timing test that cites the closed O-89). Recorded as O-94. The three that meet it are `test_gcs` (O-62), the yelp unzip (O-1) and the playground ML-100k test (O-1).

Runs in gate stages in their working form: 34 (30 vector tests, 2 cross-read, 2 godebug children through their parents). Not run in any stage: 9 (`test_gcs`, the yelp unzip, the Iris download, the crate playground test, the toml test, `math32_exhaustive`, the O-91 probe, the O-89 timing test, and the Weaviate sparse skip). The Redis TLS tests run only in their unset form. Recorded as O-95.

## Table 2: open and carried items

| id | status | what it covers | where it ends |
|---|---|---|---|
| O-1 | open | the CDN hosts for the yelp archive and the binary's own playground fetch (ml-100k and github are placed locally) | Table 1 rows 7 and 8 |
| O-30 | open | HNSW ranking parity (Go `xvec`) | WP-4-2 / vector stores |
| O-53 | open | MongoDB 8.0 not tested (ran on 6.0.x) | a later MongoDB run |
| O-62 | open | GCS read-side coverage (real-GCS run) | Table 1, `test_gcs` |
| O-73 | open | the corpus compares map-order results as multisets | WP-3-6 corpus |
| O-80 | open | the mixed test covers only the latest recommender (D-264, non-blocking) | WP-4-1 |
| O-81 | open | the mixed test has no empty-list case | WP-4-1 |
| O-83 | open | the cross-read tests leave Qdrant collections behind | `vector_cross_read.rs`, removed in WP-6-1 |
| O-84 | open | Go panics on a template list that mixes bool, string and nil | WP-4-1 |
| Q-265 | open | live check of the item-to-item and user-to-user offset past the end | WP-4-2 |
| Q-cli-5 | open | U+2028 and U+2029 without a newline (yaml.v3) | WP-4-6 |
| Q-cli-6 | open | json-patch errors that print a nested object | WP-4-6 |
| Q-cli-1 to Q-cli-4 | explained, not blocking | cobra usage text, `help`/`completion`, keyring text, yaml layout | WP-4-6 (D-294) |
| X-239 | explained, provisional, not blocking | the host name read from `/proc` only | WP-4-2 |
| X-250 | explained, upstream gap, kept for parity | OIDC sends no `state` and checks no nonce | coordinator ruling 2026-10-29 |
| X-252 | explained, upstream quirk, kept for parity | Go's paged `UpdateScores` loses trials under load | coordinator ruling 2026-10-29 |
| Q-270 | recorded | Q-WP51-1 (five table rows omit a path placeholder) and Q-WP51-2 (no update time for cold-start users) | WP-5-1 |
| X-263, X-264 | **accepted** (D-308) | the version string and the task count, by value and by JSON kind | WP-5-1 |
| O-66, O-31, O-89, O-90, O-91 | closed | see their entries | D-292, D-299, D-300, D-306, D-308 |

## Table 3: every X/Q/C/O entry in PORTING_NOTES.md

Generated from the notes by script: each row is the entry's id, the first WP token it names, and the start of its text. The full text is in the notes.

| Q-1 | - | Cache time values carry Go's monotonic clock suffix. `cache.Time` stores |
| Q-2 | - | JSON responses are one-space indented with no trailing newline |
| Q-3 | - | Router errors are go-restful's plain text `404: Page Not Found` and |
| Q-4 | - | `Last-Modified` uses Go's `time.RFC1123`, which prints `UTC`, not the |
| Q-5 | - | Two dashboard routes look up the cache key `time.Now().String()` for |
| Q-6 | - | `/api/dump` appends the JSON stats after the binary end marker and keeps |
| Q-7 | - | Dump and restore drop record metadata. Users and items are dumped |
| Q-8 | - | The OpenAPI document's `required` arrays come out in random order from |
| Q-9 | - | `Meta.config` carries every secret (database URLs with passwords, API |
| X-254a | - | pprof profile bodies are empty. `/debug/pprof/heap`, `goroutine`, `allocs`, `block`, `mutex`, |
| X-254b | - | Swagger UI is the vendored swgui v1.8.5 bundle. `/apidocs/` is the page, which Go's template renders, |
| X-254c | - | The OpenAPI document is compared with `required` sorted (Q-8). The Rust master serves the frozen |
| X-254d | - | An empty `http_cors_methods` lists the methods of the matched routes. The shipped `config/config.toml` |
| X-254e | - | The origin header follows which Go helper wrote the answer. A Go container filter adds the allowed origin |
| X-254f | - | go-restful's own 404 and 405 texts. A WebService path that no route takes answers `404: Page Not Found`, and |
| X-254g | - | Scope. The CORS filter, the OpenAPI document, Swagger UI and pprof are on the master's router. The server |
| X-215 | - | The server's CORS filter uses Go's start-time lists, which its `Sync` writes into in place. Go's |
| X-214a | - | The server serves `/apidocs.json`, Swagger UI and the pprof routes, behind the master's CORS filter. The |
| X-214b | - | `/metrics` has Go's promhttp content type. The server answers |
| X-214c | - | The golden checks. `server_http_surface` runs 5 cases against Go: the restricted CORS config, the default |
| X-214d | - | The start order of the Go server decides its default CORS lists (a Go race, found by the court, D-285). |
| X-214e | - | The harness picked ports from the ephemeral range (a harness flake, D-285). `free_port` bound port 0, which |
| Q-10 | - | `ProxyClient.GetItemStream` never sends `time_limit`, so a worker or |
| Q-11 | - | Items and users with empty `labels` bytes are rejected by the Go proxy |
| Q-12 | - | Missing records come back as OK with the optional field unset |
| Q-13 | - | Progress `error` is never transmitted: `EncodeProgress` does not set it |
| Q-14 | - | `BlobStore.ListBlobs` lists in-progress `upload-*` temp files |
| Q-15 | - | Model hyper-parameters change type across a master restart. |
| Q-16 | - | Type-mismatch log lines name the wrong type: they log |
| Q-17 | - | Blob bytes are not deterministic: user embeddings and FM scalers are |
| Q-18 | - | XLA and non-XLA CTR blobs share the `AFM`/`AFM2` header but differ in the |
| Q-19 | - | `encoding.ReadBytes` of a zero-length value depends on the reader |
| Q-20 | - | Timestamp precision differs per backend: MySQL `datetime` keeps whole |
| Q-21 | - | `users.labels` defaults to `'[]'` on MySQL and Postgres but `"null"` on |
| Q-22 | - | Full-text search columns are sorted lexicographically by their SQL |
| Q-23 | - | The Postgres cache store runs `CREATE EXTENSION IF NOT EXISTS btree_gin` |
| Q-24 | - | `database.vector_table_prefix` is applied only by xvec; the Qdrant, |
| Q-25 | - | The SQLite stores need the `json_contains` function registered on every |
| Q-26 | - | `config.MkDir` creates `$HOME/.gorse/var/lib/master` while flags are |
| Q-27 | - | Node uuids are `md5(hostname + http_host + http_port)` |
| Q-28 | - | A worker whose uuid is not yet in `Meta.workers` processes no users |
| Q-29 | - | The env binding `GORSE_DASHBOARD_AUTH_SERVER` (`master.dashboard_auth_server`) |
| Q-30 | - | The playground mode overwrites `$HOME/.gorse/etc/config.toml` on every |
| Q-31 | - | Dashboard session cookies use keys generated at master start, so every |
| Q-32 | - | `MM` adds the product into `c`, except `a·bᵀ`, which overwrites it |
| Q-33 | - | Results depend on the CPU and on `GODEBUG`. `Dot`, `Euclidean`, `MM`, |
| Q-34 | - | Go's scalar `MM` needs equal leading dimensions where the kernels do |
| Q-35 | - | `PriorityQueue.Pop` keeps the value in the lookup set. `Push` |
| Q-36 | - | `PriorityQueue.Clone` does not copy the lookup set. The copy starts |
| Q-37 | - | `PriorityQueue.Unmarshal` trusts its input. Any non-zero order byte |
| Q-38 | - | `TopKFilter` accepts NaN weights. Only `PriorityQueue.Push` rejects |
| Q-39 | - | `PriorityQueue.Pop`'s doc comment says an empty queue returns an |
| Q-40 | - | A 64-bit hash collision leaves a point without a host. `Add` |
| Q-41 | - | `GetLeast` wraps at the number of distinct points (`len(c.hosts)`), |
| Q-42 | - | Reads that write the total load. `MaxLoad` turns a total load of 0 |
| Q-43 | - | `Remove` keeps the removed host's load in the total and always |
| Q-44 | - | `MaxLoad` on an empty ring panics (integer divide by zero), after |
| Q-45 | - | `Inc`, `Done` and `UpdateLoad` ignore unknown hosts, and `Done` |
| Q-46 | - | `=` parses and matches every value. The pattern accepts `=` and the |
| Q-47 | - | A value without an operator is parsed and then ignored. `a.5` gives |
| Q-48 | - | `FromString` is not atomic and keeps the old value. The type and |
| Q-49 | - | Values of 1e6 and more, or below 1e-4, do not survive a round trip. |
| Q-50 | - | `MarshalJSON` escapes `<`, `>` and `&` (`"test\u003c16"`, asserted by |
| Q-51 | - | `FromPB` keeps `ExprType` for an unknown enum number (Go's `switch` |
| Q-52 | - | `MarshalJSON` has a pointer receiver. A value that is not addressable |
| Q-53 | - | JSON `null` is an error inside a slice but fine for the slice. |
| Q-54 | - | `Bruteforce.Add` returns the new length; `HNSW.Add` returns the new |
| Q-55 | - | `HNSW.SearchIndex` finds the query vector itself; `Bruteforce` leaves |
| Q-56 | - | An HNSW grows by at most one layer per insert. A vector drawn at |
| Q-57 | - | Layer-0 neighbour lists are cut to `maxConnection` (48), not |
| Q-58 | - | Layers come from Go's global `math/rand`. The level is |
| Q-59 | - | `Unmarshal` takes the level factor, `maxConnection`, |
| Q-60 | - | Go writes each upper layer's entries in `sync.Map` order, which |
| Q-61 | - | Searching an empty HNSW panics (`index out of range [0] with length |
| Q-62 | - | `Unmarshal` accepts layers it cannot search. A negative entry count |
| Q-63 | - | `Time.String()` ends in ` m=±<seconds>` for times from `now()` (Q-1). |
| Q-64 | - | Malformed zone data under `$ZONEINFO` is not reported. |
| Q-65 | - | `$ZONEINFO` is read once per process, at the first `LoadLocation`; |
| Q-66 | - | Indices are byte positions; after a multi-byte character the index |
| Q-67 | - | Restarts drop the options. `"Mon "`, `"Tue "` and the other weekday |
| Q-68 | - | Lowercasing is Go's `unicode.ToLower`: `İ` lowers to `i` and the |
| Q-69 | - | Each `parse()` re-applies the full month name and the weekday skip to |
| Q-70 | - | `RetryAmbiguousDateWithSwap` runs on whichever parser came back |
| Q-71 | - | An "AM" suffix is written into the layout as "PM". |
| Q-72 | - | All-digit inputs: 13- and 16-digit timestamps wrap on overflow; |
| Q-73 | - | `ParseFormat` can return invalid UTF-8. |
| Q-74 | - | "TZ offset not recognized" fires for offsets of 0, 1, 2 or 4 |
| Q-75 | - | An unknown period-month returns a non-nil parser together with the |
| Q-76 | - | amd64 `Exp` adds the exponent into its polynomial. `exp_amd64.s` has |
| Q-77 | - | amd64 `Exp` range. `k` is `x·log2(e)` rounded half to even |
| Q-78 | - | amd64 `Log` takes the exponent bits raw. `k = e - 126` with no |
| Q-79 | - | amd64 `Log` halves when `f1 <= √2/2` (`CMPSS X2, X0, 5` computes |
| Q-80 | - | amd64 `Log` of any negative input, a negative NaN included, returns |
| Q-81 | - | NaN bits. math32's `NaN()` is `0x7FE00000`, not Rust's `0x7FC00000`. |
| Q-82 | - | `Pow` and `Tanh` call `Exp`, `Log` and `Sqrt`, so on amd64 they |
| Q-83 | - | `Sin` and `Cos` above 2^29. math32's `trigReduce` adapts the float64 |
| Q-84 | - | Go's float-to-integer conversions. `int32(x)` and `int(x)` truncate |
| Q-85 | - | `\U` escapes wrap Go's 32-bit runes. The unescaper shifts |
| Q-86 | - | A lexer error replaces an earlier parser error. The lexer |
| Q-87 | - | The `if` form's node is not counted and has no location. |
| Q-88 | - | `true`, `false`, `nil` and numbers take no postfix. They |
| Q-89 | - | Map pairs are at the `{`. Every `PairNode` takes the |
| Q-90 | - | A ternary is at the token after it. The `?:` |
| Q-91 | - | `not` before an operator is matched by value and located at the |
| Q-92 | - | The lexer and the unescaper disagree on escapes. The lexer |
| Q-93 | - | Go's `Lexer.Reset` keeps the previous positions. Reusing a |
| Q-94 | - | Newlines. A quoted string cannot hold a line feed (`literal |
| Q-95 | - | Comparison chains and `?:` share operands. `a < b < c <= d` |
| Q-96 | - | An error on an empty line has no position. `file.Error` |
| Q-97 | - | Numbers. Underscores are removed anywhere before parsing |
| Q-98 | - | Disabling a builtin does not touch the predicate builtins. |
| Q-99 | - | `Sample` and `SampleInt32` count excluded values outside the |
| Q-100 | - | `NormFloat64`, `ExpFloat64` and Go's `math.Exp` follow the CPU. On |
| Q-101 | - | Go's `NaN` bits. `ParseFloat("nan")` returns Go's `math.NaN()`, |
| Q-102 | - | No host name is checked on either side of a TLS connection |
| Q-103 | - | `h2` is offered as ALPN on both sides and enforced on neither. |
| Q-104 | - | A CA limited to server authentication works in a client's CA file |
| Q-105 | - | `ParseInt` with bit size 1 drops a range error |
| Q-106 | - | go1.27 `ParseFloat` past 800 digits. `decimal.set` keeps 800 |
| Q-107 | - | Unhashable `json_contains` elements have two texts. |
| Q-108 | - | modernc's SQLite boundary. This is what Go sees, and the Rust |
| Q-109 | - | `AppendURLParams` re-encodes the whole query. The keys come |
| Q-110 | - | Raw bytes in MySQL DSN errors. Some error texts carry the |
| Q-111 | - | One expression at a time. `Unmarshal` parses one top-level |
| Q-112 | - | Swapped arguments and a double space in |
| Q-113 | - | Message texts kept verbatim, grammar included: `toml: key a |
| Q-114 | - | `nan`, `+nan` and `-nan` are all `math.NaN()`, bits |
| Q-115 | - | A leading zero at the very end of the document. The `0` |
| Q-116 | - | The space of a date-time skips a byte. `scanDateTime` adds 2 |
| Q-117 | - | Two messages for a control character in a string. A string |
| Q-118 | - | Fractional seconds past nine digits are cut, not rounded, |
| Q-119 | - | `\e` is accepted (TOML 1.1, `unstable/parser.go:773`, |
| Q-120 | - | `%c` of a byte prints the byte as the rune of the same |
| Q-121 | - | A value that starts with `}` is an `incomplete number`: |
| Q-122 | - | Key redefinition follows go-toml's tracker |
| Q-123 | - | Where an error at the end of the document is reported |
| Q-124 | - | Go's stack limit. Go's goroutine stacks grow to 1 GB; at |
| Q-125 | - | No empty final line. A trailing `\n`, `\r` or `\r\n` gives no |
| Q-126 | - | A `\r` at the end of the buffered data ends the line. |
| Q-127 | - | The opening-quote check indexes the wrong string. |
| Q-128 | - | A backslash before the last quote of a line keeps the value |
| Q-129 | - | `StrictParse` rejects every `export NAME` line. `export B` |
| Q-130 | - | Double-quoted escapes. `\n` and `\r` are replaced first, then |
| Q-131 | - | Expansion. `variablePattern` takes the braces as optional on |
| Q-132 | - | A too-long line inside a quoted value is "missing quotes". |
| Q-133 | - | x/text's decoders. UTF-16: a lone surrogate is U+FFFD per |
| Q-134 | - | `\r` alone ends a line, inside quotes too (`A="x\ry\r"` → |
| Q-135 | - | Separators. `=` takes white space on both sides; `:` needs at |
| Q-136 | - | Comments and trailing text. `#` starts a comment anywhere in |
| Q-137 | - | Invalid UTF-8 is a non-space rune. `TrimSpace` stops at an |
| Q-138 | - | Shape checks. `add`, `sub`, `mul`, `div` and `pow` require |
| Q-139 | - | Reads past the end. `embedding` and `slice_indices` read past |
| Q-140 | - | Shared data. `slice`, `flatten` and `reshape` share their data |
| Q-141 | - | `Load` panics. Where Go panics while loading a model, `load` |
| Q-142 | - | Go's runtime messages for index and slice bounds (`golang.rs`, |
| Q-143 | - | Shortest float digits settle ties to even. Go's shortest float |
| Q-144 | - | Case mapping is Go's, not Rust's. viper lower-cases keys with Go's |
| Q-145 | - | Duplicate keys that differ only by case. `config.toml` with |
| Q-146 | - | A file value shadowing an env-bound key. `master = 5` in the file |
| Q-147 | - | an unmarshal error for a `!!map` or `!!seq` tag includes the raw |
| Q-148 | - | a latched scanner error is reported on parser entry and again |
| Q-149 | - | a reader error is raised only when the scanner needs characters |
| Q-150 | - | a scanner error message gets one extra line, as yaml.v3's |
| Q-151 | - | comment marks decide where BLOCK-END tokens fall |
| Q-152 | - | A per-minute limit from 1 to 59 stops the master. The bucket |
| Q-153 | - | Limits of zero or less keep the previous limiter, and a very |
| Q-154 | - | `Split(a, 0)` with a non-empty `a` panics (integer division by |
| Q-155 | - | `Parallel` reports the lowest failing job, not the first |
| Q-156 | - | `Detachable` with `nWorkers` 0 blocks until the context is |
| Q-157 | - | juju's int64 arithmetic wraps. `Take(math.MaxInt64)` while |
| Q-158 | - | juju's clock going backwards gives negative ticks, which |
| Q-159 | - | juju's `NewBucketWithRate` test never runs: it is a gocheck |
| Q-160 | - | `sizeof` charges unused slice capacity at 24 bytes per slot. |
| Q-161 | - | The decision client accepts any 2xx; the reranker client accepts only 200. |
| Q-162 | - | Error texts are Go's. Decision: `jev request failed with status N: <body, trimmed>`. |
| Q-163 | - | Optional request fields are omitted as in Go. `top_n` is left out when zero |
| Q-164 | - | The chat mock prefixes replies for model `deepseek-r1` with a `<think>` block |
| Q-165 | - | Stream pieces are cut by bytes, as Go cuts them. Eight bytes per piece. A piece can |
| Q-166 | - | Embedding input must be a string. Go answers 400 with `invalid input type`, and the |
| Q-167 | - | `GetString` panics on any other type. Go's `val.(string)` panics |
| Q-168 | - | The download keeps any body. `http.Get` does not check the |
| Q-169 | - | A failed download reports the unzip error. |
| Q-170 | - | Only a missing file downloads. `os.IsNotExist(err)` on the |
| Q-171 | - | The format error. The message is `format not matchs %v != %v` |
| Q-172 | - | Absolute entry names stay inside the destination. Go's |
| Q-173 | - | `Params` type mismatch log. Q-16 applies: the log names |
| Q-174 | - | JSON names are the Go field names. The records have no JSON tags, |
| Q-175 | - | Map names are written sorted. `encoding/json` sorts a map's names, |
| Q-176 | - | An error text lists the labels as JSON. |
| Q-177 | - | Key matching is case-sensitive, and duplicate names are an error. |
| Q-178 | - | REST labels need a `UseNumber` decode. `Deserialize` gives |
| Q-179 | - | Nesting depth. Go's `encoding/json` allows 10,000 levels of |
| Q-180 | - | Go's map order in `ValidateLabels`. Go checks map entries in |
| Q-181 | - | Go's native `int` labels. Go code can put an `int` into labels, |
| Q-182 | - | `mapstructure` tags are not ported. The record types carry |
| Q-183 | - | `sort.Slice` is not stable. Score documents with equal scores |
| Q-184 | - | `Open` chooses among matching prefixes by map order in Go. The |
| Q-185 | - | `Time` values are written and read as Go does. The writer is |
| Q-186 | - | Empty requests skip the collection check. `AddVectors` with no |
| Q-187 | - | Query and get timestamps differ. `GetVectors` returns |
| Q-188 | - | `DeleteCollection` restores on failure. If xvec's `Destroy` |
| Q-189 | - | `Close` is idempotent, and every call after it fails with |
| Q-190 | - | `DeleteVectors` removes timestamps strictly before the cutoff. |
| Q-191 | - | `GetVectors` returns vectors in the order of the ids, each id |
| Q-192 | - | `NoDatabase.Optimize` succeeds. Every other `NoDatabase` method |
| Q-193 | - | one timestamp per chunk. Go stamps every chunk with its own time, |
| Q-194 | - | 4 MiB chunks exceed the gRPC limit. A 4 MiB chunk plus its framing |
| Q-195 | - | An empty write of time series points is an error. `AddTimeSeriesPoints` |
| Q-196 | - | `Purge` leaves the time series points. Go purges the values and the |
| Q-197 | - | Scan loops swallow cursor errors. `Scan`, `SearchScores` and |
| Q-198 | - | Times are stored in milliseconds, with Go's formula. Go writes |
| Q-199 | - | `IsHidden` is always false when read. Go's BSON decoder reads the |
| Q-200 | - | Go sends one `$set` per patched field. `UpdateScores` with two or |
| Q-201 | - | A negative `begin` is refused by the server. The driver's skip is |
| X-1 | - | xvec on-disk format (`PORTING.md` D-6). The Rust master implements the |
| X-2 | - | BlobStore names are checked (`PORTING.md` D-12). The Go server joins |
| X-3 | - | `GetFeedbackStream` without `scan_options` (`PORTING.md` D-13). The Go |
| X-4 | - | `/debug/pprof/*` (`PORTING.md` D-14; the master part is X-254). A Rust process cannot produce Go |
| X-5 | - | No `go_*` Prometheus metrics. The Go client registers Go runtime |
| X-6 | - | `--help` layout. Flags, defaults and behaviour match |
| X-7 | - | Corrupt lengths are errors (`PORTING.md` D-23). Go's `ReadBytes` and |
| X-8 | - | gob values Gorse never writes (`PORTING.md` D-23). Go's gob decoder |
| X-9 | - | `common/floats` beyond the release builds' behaviour (`PORTING.md` |
| X-10 | - | `pop` and `peek` on an empty `PriorityQueue` return `None` where Go |
| X-11 | - | `PriorityQueue::unmarshal` returns a new queue. Go's `Unmarshal` |
| X-12 | - | `TopKFilter::new` takes `usize`. Go takes an `int`; a negative `k` |
| X-13 | - | Methods promoted from Go's unexported `_heap` are not ported. `Less` |
| X-14 | - | The consistent-hash ring has no internal lock. Go's `Consistent` |
| X-15 | - | The ring's exported `Host` struct is not a public type. Loads are |
| X-16 | - | Ring panic texts differ. Where Go panics with a runtime error |
| X-17 | - | `MaxLoad` above `i64::MAX`. It converts `ceil(total / hosts * 1.25)` |
| X-18 | - | Ring keys and hosts are `&str`. Go strings may hold invalid UTF-8; |
| X-19 | - | `unmarshal_json` of text that is not JSON returns serde_json's |
| X-20 | - | serde errors for feedback type expressions carry serde_json's position |
| X-21 | - | `Hnsw::add` takes `&mut self`. Go's HNSW lets goroutines insert and |
| X-22 | - | `Hnsw::unmarshal` is atomic and never panics on counts. Go replaces |
| X-23 | - | An insert into an unmarshalled empty index. Go deadlocks: the index |
| X-24 | - | Narrower argument types. `q` and `k` are `usize`: Go's negative `q` |
| X-25 | - | The monotonic clock starts at first use, not at process start: Go |
| X-26 | - | No build-time GOROOT. Go's `runtime.GOROOT()` falls back to the GOROOT |
| X-27 | - | `time/tzdata` (the embedded zone database) is not ported: Gorse does |
| X-28 | - | A truncated zip central directory is reported as `corrupt zip file`; |
| X-29 | - | `serde::Deserialize` for `Time` receives the unescaped string, where |
| X-30 | - | Errors are `Result`s. Go returns `(zero Time, err)`; the ported Go |
| X-31 | - | dateparse API shapes: `parse_in` takes `&Location` (Go's |
| X-32 | - | Time panics. Where Go panics (index out of range, nil parser), the |
| X-33 | - | math32 on arm64 is not bit-exact. There math32's `Exp` and `Log` run |
| X-34 | - | Only the math32 functions Gorse calls are ported: `Abs`, `Copysign`, |
| X-35 | - | `Sqrt` on targets other than x86-64 and aarch64 uses the Go code. Go |
| X-36 | WP-1-4 | A TLS client whose chain does not reach the server's CA (WP-1-4). |
| X-37 | WP-1-4 | TLS handshake error texts are rustls's, not Go's (WP-1-4, |
| X-38 | - | No stack limit. Where Go dies (Q-124), the port |
| X-39 | - | Floats with more than 800 digits before the point: closed |
| X-40 | - | Bytes that are not UTF-8 in a value or an error text. Go |
| X-41 | - | `add` of a tensor without elements. Go loops forever (`i += 0`). |
| X-42 | - | Benchmarks not ported. `tensor_test.go`'s `matMulLegacy`, |
| X-43 | - | Progress bar. `TestMNIST`'s progress bar is not ported. The |
| X-44 | - | Go's top-level `rand` is a stand-in (D-81): splitmix64 |
| X-45 | - | Dataset tests ignored. `TestIris` and `TestMNIST` need files |
| X-46 | - | `NewTracerProvider` is not ported. It builds the OpenTelemetry |
| X-47 | - | YAML cases were ignored, now un-ignored: closed. `config::tests::test_unmarshal_yaml` |
| X-48 | WP-1-8 | Log output: closed. Until WP-1-8 merged, `LoadConfig` had a placeholder logger (D-84). The log-swap merge (0df |
| X-49 | - | (closed 2026-10-28) `contracts/config/config.schema.json` and `config.keys.md` are not |
| X-50 | - | (non-UTF-8 bytes): a Go string is bytes. Only a `!!binary` |
| X-51 | WP-1-6 | `gocontext` is partial (WP-1-6). Deadlines and timeouts, causes, `AfterFunc` and `WithoutCancel` are not porte |
| X-52 | - | Log format and the loggers not ported. Log lines are tracing's |
| X-53 | - | `json` NaN and infinity. Go's `json.Marshal` fails with |
| X-54 | - | `copier` aliasing and errors. Go's copy can leave the copied |
| X-55 | - | Home directory. Go takes the home directory from |
| X-56 | - | `datautil` details. A short CSV row (fewer than five fields) is |
| X-57 | - | Monitor and `sync.Map`. Each span guards its fields with a |
| X-58 | - | The OpenAI mock listens on 127.0.0.1, not on every interface. Go's |
| X-59 | - | The mock's JSON leaves out two fields. Go writes `content_filter_results` and the |
| X-60 | - | `start()` returns `Ok(())` after `close()`. Go returns `http.ErrServerClosed`. |
| X-61 | - | Chunked request bodies get a 400. Go's server reads them. The Gorse clients send |
| X-62 | - | `"answers": null` is a decode error. Go's nil map accepts it. Serde rejects `null` for |
| X-63 | - | Float32 embeddings are written as `139.0`. serde_json writes an `f32` with a fraction; |
| X-64 | - | An empty `messages` list gets a 500. Go indexes `Messages[0]` without a check, and |
| X-65 | - | `SuggestParams` is not in the `Model` trait. Go's |
| X-66 | - | `init` side effect. Go's `init` creates `~/.gorse/dataset` and |
| X-67 | - | File permissions on unzip. Unzipped files get default |
| X-68 | - | `GetRandomGenerator` before `SetParams`. Go returns a zero |
| X-69 | - | `Copy` does not share slices. Go's `Copy` is shallow, so an |
| X-70 | - | `unzip` returns only the error. Go returns the names it wrote |
| X-71 | - | Strings are `String`, not bytes. D-56 treats Go strings as bytes |
| X-72 | - | A test deadline is a cancel. `gocontext` has no deadlines. The |
| X-73 | - | `Integer` returns only an error. Go's `ReturnValue.Integer` |
| X-74 | - | Float16 bytes are copied. Go's `Float16Bytes` and |
| X-75 | - | Exact search, not HNSW. Go's `xvec` answers queries with an |
| X-76 | - | The xvec format is Rust's own (refines X-1). X-1 says a master |
| X-77 | - | Collection names are checked. The empty name, `.`, `..`, and |
| X-78 | - | The collection schema is fixed at creation. `DescribeCollection` |
| X-79 | - | S3 close returns the upload error. Go's `S3.Create` runs |
| X-80 | - | list strips the prefix the same way. Go's S3 `List` slices |
| X-81 | - | temp file removed on failure. A failed master upload deletes its |
| X-82 | - | write errors are returned. POSIX and master writes return their |
| X-83 | - | EOF on the next read. Go's minio reader returns `io.EOF` with the |
| X-84 | - | missing env panics. The cloud tests panic when a required variable |
| X-85 | - | buckets are created out of band. object_store has no |
| X-86 | - | constructors return a Result. `MasterStoreServer::new` returns |
| X-87 | - | cancelled cloud multipart. A cancelled write leaves an unfinished |
| X-88 | - | ctx on every method. `Store` and `BlobWriter` take `ctx`, as D-122 |
| X-89 | - | Go's `NewScanOptions` skips nil options. A `ScanOption` cannot be nil in Rust, so there is nothing to skip. Be |
| X-90 | - | The shared suite's faker labels are fixed strings. Go's `faker` colour and genre names are replaced with fixed |
| X-91 | - | Go's `[]string` and `[]any` labels are one `Labels::Array`. Go's conversions to `[]any` in the suite (`feedbac |
| X-92 | - | The suite's type switches are a `Backend` trait. Go's `suite.Database.(*SQLDatabase)` and `*MongoDB` checks be |
| X-93 | WP-1-17 | Go's `TestSortFeedbacks` and `TestValidateLabels` are in `types.rs`. WP-1-17 ported them, so the suite file do |
| X-94 | - | Go's `time.Sleep(time.Millisecond)` is `std::thread::sleep` in the suite. The suite runs on the test's own thr |
| X-95 | - | `connect=direct` is read as `directConnection=true`. Go's |
| X-96 | - | A nil slice of categories is stored as an empty array. Go writes a |
| X-97 | - | The context is checked on entry and between documents. Per D-122: |
| X-98 | - | No OpenTelemetry command monitor. Go sets |
| X-99 | - | Each write is its own round trip. Same results, more round trips |
| O-1 | - | The Go CI's model tests read the datasets from `https://cdn.gorse.io`, |
| O-2 | - | prost leaves `-0.0` out of proto3 `double` and `float` fields; |
| O-3 | - | math32's `Log1p` and `Log2` (`model/cf` and `model/ctr`) are |
| O-4 | - | `common/expression` has its own `ParseFloat` for the strings its |
| O-5 | - | `rustls`'s own unit tests do not build from the published package |
| O-6 | - | The SQLite stores register `json_contains` as deterministic, |
| O-7 | - | The MySQL stores call `probe_mysql_isolation_variable_name` |
| O-8 | - | Dropping very deep results. The compiler's drop of a |
| O-9 | - | `gostrconv::parse_float` and Go's 800-digit decimal: closed. |
| O-10 | - | `cargo doc` fails on the base commit: closed. `src/value.rs` |
| O-11 | - | Environment values that are not UTF-8. Go's `os.LookupEnv` |
| O-12 | - | Merge points: resolved. The `Cargo.toml` (regex, stacker, and the |
| O-13 | - | `parallel.For` stand-in: closed. `deps::parallel_for` (see For the |
| O-14 | - | `GODEBUG` and `util::rand::math`. `util::rand::math::use_fma` |
| O-15 | - | Dataset tests (X-45) run once `cdn.gorse.io` is reachable, |
| O-16 | - | `gorse-expr` keeps its own copies of Go's strconv pieces. `gorse-expr/src/gostr.rs` |
| O-17 | WP-4-3 | Who owns `NewTracerProvider`? Decided by the integrator on 2026-10-28. The Go method is `config/config.go:596` |
| O-18 | - | yaml.v3 compacts its token queue when the backing array is |
| O-19 | - | `is_bom` checks the BOM at the start of the decode window |
| O-20 | - | (not exercised): the derived `Clone`, `PartialEq` and `Debug` of |
| O-21 | WP-1-8 | `TestLoadIris` is ignored (WP-1-8). It downloads from `cdn.gorse.io`, which is blocked here, as with O-1. It i |
| O-22 | WP-1-6 | A test flake and a `detachable` race (WP-1-6): closed. `for_and_detachable_panics_resume_on_the_caller` failed |
| O-23 | - | `TestUnzip` ignored. It downloads `yelp.zip` from |
| O-24 | - | `TestLocateBuiltInDataset` ignored. It downloads `ml-1m` from |
| O-25 | WP-1-8 | `datautil` download has a 30 second timeout (WP-1-8): closed. Go's `http.Get` has none, and `datautil.rs` call |
| O-26 | - | pdqsort's heapSort fallback is not reached by any golden case. |
| O-27 | WP-2-4 | (closed by f7ff3189) The cache core copies Go's pdqsort (WP-2-4). `cache/gosort.rs` is a |
| O-28 | - | The shared suite has not run against a backend. No backend |
| O-29 | - | Benchmarks are not ported. `benchmark`, `benchmarkAddDocuments`, |
| O-30 | - | HNSW ranking parity. Go's `xvec` answers with HNSW, which is |
| O-31 | - | (closed by 3966bf6f) GCS write cannot run on fake-gcs-server. The XML `PUT` to the |
| O-32 | - | fake-gcs-server needs `-external-url` for path-style XML. Without |
| O-33 | - | live emulators. RustFS runs as the `rustfs/rustfs:alpha` container |
| O-34 | - | a missing blob over gRPC is `Unknown`. Go's server returns the |
| O-35 | WP-1-8 | `log` rotation can overwrite a backup within one millisecond (WP-1-8). Backup |
| O-36 | WP-2-2 | The suite is compiled and not run. `data/database_test.rs` is called by no test in this crate. WP-2-2 (SQL) an |
| O-37 | WP-2-1 | (closed by the WP-2-1 merge) Merged with the integration head 9c89272 (f8b3662). The cache and vector cores ar |
| O-38 | - | Crate-selection rows. `porting/crate-selection.md` needs rows for `async-trait` (0.1.92), `futures-core` and ` |
| O-39 | - | TestTimezone loads `Asia/Tokyo` through `gotime::load_location`, which reads the system zoneinfo. The case nee |
| O-40 | WP-2-2 | Shared-suite cases that need a backend are untested here. Every case in `database_test.rs` is unverified until |
| O-41 | - | No callback in the data trait takes a `&str`. The only `Fn` in this WP is `ScanOption`, which takes `&mut Scan |
| O-42 | - | Opening the store in a node. Closed by D-151 (integration). The |
| O-43 | - | The test without `MONGO_URI`. The test returns early and reports |
| O-44 | - | MongoDB versions tested. Only 6.0.28 (the `mongo:6` image, the |
| O-45 | - | (closed by f7ff3189) `gosort` copy. This store does not sort in Rust (MongoDB sorts |
| Q-202 | - | `UpdateScores` checks, then writes, with no transaction. Go's |
| Q-203 | - | `ScanScores` fails on a key deleted between `SCAN` and `HGETALL`. |
| Q-204 | - | A search with `begin == end == 0` has no `LIMIT`. go-redis sends |
| Q-205 | - | `Scan` and `Purge` also visit score documents and points. Both match |
| Q-206 | - | A category token that decodes to nothing panics in Go. |
| Q-207 | - | A missing time series is matched by text. Go checks for the |
| X-104 | - | `Close` drops the connection. The `ConnectionManager` closes with the |
| X-105 | - | Cancellation is checked at call boundaries. go-redis aborts a command |
| X-106 | - | `CLIENT SETNAME` is sent once, on the first connection. go-redis names |
| X-107 | - | A cluster sends pipelines one command at a time. redis-rs refuses a |
| X-108 | - | Cluster primaries come from one `CLUSTER NODES` reply. Go's |
| X-100 | - | Cluster `addr` parameters take the base URL's credentials. Go's |
| X-101 | - | Each test owns a table prefix and purges it, not `FLUSHDB`. Go's suite |
| X-102 | - | `rediss://` connects through rustls, with the system roots (was X-81 |
| X-103 | - | `skip_verify=true` is honoured on `rediss://`, as go-redis does |
| Q-208 | - | go-redis's `skip_verify` turns verification off, and the port now does |
| O-46 | - | The Redis acceptance runs pass on Redis 8 and Kvrocks. The |
| O-47 | WP-2-5 | The Go CI's Redis jobs are covered for WP-2-5. The standalone, |
| X-109 | - | A nil `Params` encodes as `{}`, not `null`. `Params` is a |
| X-110 | - | `ToJSON` returns `Result`. Go's `lo.Must1` panics. The |
| X-111 | - | `FromJSON` matches JSON keys case-sensitively. Go's |
| X-112 | - | A type mismatch stops the decode. Go's `json.Unmarshal` keeps |
| X-113 | - | Error texts are not Go's. `encoding/json` texts (syntax, type |
| X-114 | - | `Open` connects now; Go's `sql.Open` connects on first use. An |
| X-115 | - | The DSN's query string is not parsed. The file is the part |
| X-116 | - | One connection, not a pool. Calls are serialised, where Go's |
| Q-209 | - | `Equal` does not panic on slice parameters. Go's `maps.Equal` |
| O-48 | - | Resolved by D-166. The coordinator approved a `ParamValue` |
| O-49 | - | Phase 3 must give `cf.Score` and `ctr.Score` Go's encoding. |
| O-50 | - | Node JSON for the REST API is not defined here. Go's `[]meta.Node` in |
| Q-210 | - | `floats.FromAny` on an empty list. An empty `[]any` label is |
| Q-211 | - | Negative `FreqDict` indices panic. `String`, `Freq` and the lock |
| Q-212 | - | `Index.Unmarshal` drops repeated names. It rebuilds with `Add`, so |
| Q-213 | - | `SampleUserNegatives` caches. The first call's samples are kept |
| Q-214 | - | `SplitCF` with `numTestUsers <= 0` selects every user as a test |
| Q-215 | - | `GetItems` of a `UnifiedDirectIndex` logs an empty warning. Ported |
| Q-216 | - | The error text `wrong foramt` keeps Go's misspelling. |
| Q-217 | - | Ties in `SplitLatest`. Go's `sort.Slice` is the pdqsort that |
| X-117 | - | `FreqDict.ToIndex` copies. Go's `Index` shares the dictionary's |
| X-118 | - | Splits and the built-in test set copy the dictionaries. Go shares |
| X-119 | - | `FreqDict` locks are not `sync.Mutex`. Each index has a flag under |
| X-120 | - | Label ID order inside one map. Go iterates a `map[string]any` in |
| X-121 | - | `strutil.Pool` is dropped. `processLabels` interns map keys |
| X-122 | - | Malformed dataset lines are errors, not panics. A line with too |
| X-123 | - | A negative length in a serialised index is an error. Go's |
| X-124 | - | `bufio.Scanner`'s 64 KB line limit is not reproduced. Go's |
| X-125 | - | Nil receivers are `Option`s. Go's `*Index` may be nil and then |
| X-126 | - | Traits and concrete types. `CFSplit` and `CTRSplit` are Rust |
| X-127 | - | Dataset labels. Go's `data.User` and `data.Item` hold `any` |
| X-128 | - | Names. Go's `ID` is `Id`, `NotId` is `NOT_ID`, `CFSplit` is |
| X-129 | - | Go `int` parameters. `numTestUsers` and `shots` are `i64`, which |
| O-51 | - | `floats.FromAny` is not in `gorse-common`. Go's `FromAny` takes any |
| O-52 | - | `CtrSplit` has no implementation. Its implementations are in |
| Q-218 | - | `GetFeedback` with neither bound finds nothing. Go always sets |
| Q-219 | - | `GetItemStream` keeps items strictly after `timeLimit`. Go uses |
| Q-220 | - | The three streams ignore the context. They run on |
| Q-221 | - | Scan loops ignore cursor errors. Go's loops never check `r.Err()` |
| Q-222 | - | `Reconcile` rebuilds the text index on every call. `listIndexes` |
| Q-223 | - | JSON number labels are stored as numbers, not text. The Go |
| Q-224 | - | Feedback cursor JSON. The cursor is the base64 of |
| Q-225 | - | Cursors accept non-zero trailing bits. Go's `StdEncoding` decodes |
| Q-226 | - | Upserts write whole structs. `BatchInsertUsers` and |
| Q-227 | - | Feedback merge. With `overwrite` false, the upsert uses `$inc` |
| Q-228 | - | Field names. Go's BSON codec lower-cases the field names, so the |
| X-130 | - | No `bulk_write`. The Rust driver's `bulk_write` needs MongoDB 8.0, |
| X-131 | - | `GetItems` and `GetUsers` with `n = 0`. Go reads `items[n-1]` when |
| X-132 | - | Labels of an array's elements. Go's `unpack` does not unpack the |
| X-133 | - | Labels that are int32 or int64. Go keeps the typed value. `Labels` |
| X-134 | - | Labels of other BSON types. A date, an ObjectId or a binary in a |
| X-135 | - | `ErrNotFound` text. Go wraps `storage.ErrNotFound` as |
| X-136 | - | Cursor keys match case-sensitively. Go's `json.Unmarshal` matches |
| X-137 | - | `Close` returns `Ok` after shutdown. The driver's `shutdown` gives |
| O-53 | - | MongoDB 8.0 is not tested. The suite ran on MongoDB 6.0.x, the |
| O-54 | - | (closed at merge) The trait doc for `get_item_stream` was wrong for both stores. It |
| O-55 | - | The `connect` option mapping is copied. `driver_uri` (the Go |
| O-56 | - | (closed at merge) No built-in registration yet. `mongodb::register` registers the |
| X-138 | - | DSN options with no effect. Timeouts, `charset` and `loc` are |
| X-139 | - | TLS. `sqlx`'s default TLS differs from `lib/pq`'s |
| X-140 | - | Sub-microsecond timestamps. Postgres rounds in `lib/pq`; `sqlx` |
| X-141 | - | Buffered scans. `scan` and `scan_scores` read all rows before the |
| X-142 | - | NULL decodes to zero. A NULL in a non-pointer column reads as the |
| X-143 | - | One SQLite connection. Go's pool of connections is one connection |
| X-144 | - | MySQL full authentication over plaintext (resolved, matches Go). On a |
| X-153 | - | Pre-release RSA dependency (D-182). `mysql-rsa` pulls `rsa` 0.10.0-rc.19, and its |
| Q-229 | WP-2-13 | Schema equality (closed by WP-2-13, D-193). The tables this store creates equal |
| Q-230 | - | rusqlite version. Resolved by D-160: keep 0.39.0. |
| Q-231 | WP-2-2 | Shared glue. WP-2-2 may land its own `sqlconn`. Reconcile at merge |
| Q-232 | - | MySQL auth parity (X-144). Resolved by the coordinator's decision: match Go. |
| Q-233 | - | PostgreSQL `Reconcile` rebuilds an unchanged search index. Resolved, and Go does the same. The server rewrites |
| Q-234 | - | SQLite `Reconcile` rebuild. Resolved, and matches Go. A sentinel row inserted into `gorse_search_index` surviv |
| Q-235 | - | labels NULL versus "null". Go writes a nil feedback label as SQL `NULL`. Items and users store the JSON text ` |
| Q-236 | - | time text per driver. MySQL truncates `time_stamp` to whole seconds. `update_at` is `datetime(6)` and keeps mi |
| Q-237 | - | SQLite time comparison is textual. Go's `time_stamp <= DATETIME()` compares text, so rows in the same second w |
| Q-238 | - | counts are estimates on MySQL and PostgreSQL. Go's `Count*` reads `SHOW TABLE STATUS` (MySQL) and `reltuples`  |
| Q-239 | - | one statement per batch. Each write batch is one `INSERT`, as the Go code in this file writes it. Batches larg |
| Q-240 | - | GCS, S3 and Azure list by the raw string prefix. Go's `List` passes the prefix to the store as a plain string  |
| X-145 | - | `Init` creates, it does not migrate. Go's `Init` runs GORM `AutoMigrate`, which also alters tables that alread |
| X-146 | - | `ErrNotFound` is the bare sentinel. Go wraps it with the id (`item X not found`). The port returns `Error::Sto |
| X-147 | - | pool size defaults differ. Go's `MaxOpenConns` unset means no limit. sqlx's default is 10 when the option is n |
| X-148 | - | `MaxIdleConns` is not applied. sqlx has no cap on idle connections, so the option is ignored. `ConnMaxLifetime |
| X-149 | - | MySQL DSN parameters. The store adds `sql_mode`, the isolation variable and `parseTime` as Go does. Every othe |
| X-150 | - | SQLite URL parameters. The file is the part before `?`. The two pragmas Go adds (`busy_timeout(10000)`, `journ |
| X-151 | - | PostgreSQL TLS default. lib/pq requires TLS when `sslmode` is unset. sqlx defaults to `prefer`. This is not ch |
| X-152 | - | the MySQL timezone case reports a pass. Go's `testTimezone` calls `Skipf` for MySQL. The shared suite here ret |
| O-57 | - | (closed at merge) the Go-to-Rust read path. Run, and it matches. Go populates a database and Rust reads it, an |
| O-58 | - | O-36. The brief asks for the shared suite against each backend, which closes O-36 with the MongoDB data store. |
| O-59 | - | (closed at merge) Q-233 and Q-234 need Go runs. Done, see above. Nothing is open here. |
| O-60 | WP-2-2 | Two SQLite drivers in one build. The SQL data store (WP-2-2) reads SQLite through |
| O-61 | - | `mysql_cold_full_authentication_connects` fails on MariaDB 10.2. The test (X-144, |
| O-62 | - | GCS read-side coverage. The read side is covered in process and checked request by |
| O-63 | WP-2-13 | Samples and the Redis key layout are not compared. WP-2-13 compares the schema |
| O-64 | WP-2-13 | Mixed-version runs. The WP-2-13 run used the Go and Rust builds at the pinned commit, |
| X-154 | WP-2-9 | Milvus index parameters are unnested (WP-2-9, deviation 1). The REST gateway returns an |
| X-155 | - | Milvus `description` goes at the top level of the create request (deviation 2). A |
| X-156 | - | Milvus category filters and query-by-ids are literals with JSON escapes (deviations 3 |
| X-157 | - | Milvus waits by polling (deviation 5). Go's `Await` becomes a 100 ms poll of |
| X-158 | - | Milvus sparse vectors travel as JSON objects (deviation 6). They map dimension to value, |
| X-159 | - | Weaviate uses REST and has no sparse vectors (deviation 7). Go's `Open` sets no gRPC |
| X-160 | - | Qdrant `delete_collection` errors for a missing collection (deviation 8). It checks the |
| Q-240 | - | Turbo-quant bit names (deviation 9). Go's `TurboQuantBitSize` enum has no 8-bit value, |
| O-65 | - | The Weaviate sparse case is ignored. Weaviate has no sparse vectors in this port, and |
| O-66 | - | Backend tests with no server configured must not pass (court audit, 2026-10-28). The |
| X-161 | WP-2-12 | Contexts do not cross the wire (WP-2-12, deviation 1). Go passes the call's context |
| X-162 | WP-2-12 | Where Go panics, the Rust server returns an error (WP-2-12, deviation 2). Go's server |
| X-163 | WP-2-12 | The cache scan buffer is unbounded (WP-2-12, deviation 3). `scan_scores` on the Rust |
| X-164 | WP-2-12 | Malformed labels JSON uses serde's text (WP-2-12, deviation 4). A labels blob that is |
| X-165 | WP-2-12 | The brief's base commit was an ancestor of the integration head (WP-2-12, deviation 5). |
| Q-241 | WP-2-12 | Timestamps of nil read as Go reads them (WP-2-12, quirk 1). `time_from_pb` of a nil |
| Q-242 | WP-2-12 | `GetItemStream` drops `time_limit` (WP-2-12, quirk 2). Go's client does not send it, |
| Q-243 | WP-2-12 | Empty repeated fields are nil on the wire (WP-2-12, quirk 3). An empty `categories` |
| Q-244 | WP-2-12 | The cache `Get` hides backend errors (WP-2-12, quirk 4). Go's server answers with no |
| Q-245 | WP-2-12 | An unknown distance is refused with Go's text (WP-2-12, quirk 5). A distance number |
| O-67 | WP-2-12 | One scenario, two languages (WP-2-12, oracle 1). `contracts/tools/proxygolden` runs a |
| O-68 | WP-2-12 | Both directions are checked on the wire (WP-2-12, oracle 2). |
| O-69 | WP-2-12 | The transcript does not cover search or the vector stores (WP-2-12, oracle 3). Closed by D-218. The follow-up  |
| Q-246 | WP-3-4 | An empty first vector leaves the dimension at 0 (WP-3-4, quirk 1). `Add` sets the |
| Q-247 | WP-3-4 | Duplicate indices in a sparse vector all reach the pruner (WP-3-4, quirk 2). |
| Q-248 | WP-3-4 | NaN in a sparse vector (WP-3-4, quirk 3). An id whose idf is NaN passes the `idf <= 0` |
| Q-249 | WP-3-4 | A sparse vector with more indices than values panics (WP-3-4, quirk 4). Go indexes past |
| Q-250 | WP-3-4 | Searching an empty items index panics (WP-3-4, quirk 5). Go's `knnSearch` panics, as |
| X-166 | WP-3-1 | `math32.Log2` is a stand-in (WP-3-1, deviation 1). Go's `math32.Log2` is |
| X-167 | WP-3-1 | A cancelled fit keeps the last completed phase (WP-3-1, deviation 2). Go updates the |
| X-168 | WP-3-1 | A failed `unmarshal` leaves the model unchanged (WP-3-1, deviation 3). Go's `Unmarshal` |
| X-169 | WP-3-1 | A negative count counts as 0 (WP-3-1, deviation 4). Go panics when a negative |
| X-170 | WP-3-1 | Unregistered parameter types fail with Go's text (WP-3-1, deviation 5). Go's gob |
| X-171 | WP-3-1 | `evaluate` takes the test set by `&mut` (WP-3-1, deviation 6). Go's |
| X-172 | WP-3-1 | The model types are split (WP-3-1, deviation 7). Go's `Model` interface is `cf::Model` |
| X-173 | WP-3-4 | `MatrixFactorizationItems::add` takes `&mut self` (WP-3-4, deviation 1). Go's `Add` |
| X-174 | WP-3-4 | The items blob's timestamp is a gob `time.Time` (WP-3-4, deviation 2, resolved). The |
| X-175 | WP-3-4 | `VectorWriter::clean` with no store returns an error (WP-3-4, deviation 3). Go calls |
| X-176 | WP-3-4 | Items and users `unmarshal` replace all or nothing (WP-3-4, deviation 4). Go's `make` |
| X-177 | WP-3-4 | The sparse pruner's heap is a local `container/heap` port (WP-3-4, deviation 5). |
| X-178 | WP-3-4 | `search` takes `n: usize` (WP-3-4, deviation 6). Go's `int` may be negative, and a |
| O-70 | WP-3-1 | CDN-hosted dataset tests and metric parity (WP-3-1, open question 1, O-1). |
| O-71 | WP-3-1 | The replay uses a synthetic split (WP-3-1, open question 2). The replay fits 40 users, |
| Q-251 | WP-3-2 | Go's FM weights are random unless `GODEBUG=randautoseed=0` (WP-3-2, quirk 1). Go's FM |
| Q-252 | WP-3-2 | `predict` and `internal_predict` panic on the AFM (WP-3-2, quirk 2). Go's `Predict` and |
| Q-253 | WP-3-2 | Early stopping keeps the first greatest score (WP-3-2, quirk 3). Go's early stopping |
| X-179 | WP-3-2 | `Afm::parameters` is `pub(crate)` (WP-3-2, deviation 1). The replay test reads the |
| X-180 | WP-3-2 | The golden dataset has no timestamps (WP-3-2, deviation 2). The FM fit never reads them, |
| X-181 | WP-3-2 | The golden stores the train and test sets as explicit tables (WP-3-2, deviation 3). The |
| X-182 | WP-3-2 | `set_params_reads_the_hyper_parameters_with_go_defaults` is an addition (WP-3-2, |
| X-183 | WP-3-2 | `evaluator_test.go` was ported late (WP-3-2, deviation 5). Its three tests were missed in |
| O-72 | WP-3-2 | The metrics conformance test needs the datasets (WP-3-2, open item). The WP-3-8 metric |
| Q-254 | WP-3-6 | Non-personalised pushes from parallel workers need `Send` expressions (WP-3-6, closed by D-232). |
| Q-255 | WP-3-6 | `json.Number` labels become floats in expressions (WP-3-6, open). Go keeps a |
| Q-256 | WP-3-6 | Each `push` converts the item and all its feedback (WP-3-6, measure later). The cost is |
| X-184 | WP-3-6 | `NonPersonalized` is not `Send` (WP-3-6, deviation 3). See Q-254 and D-227. |
| X-185 | WP-3-6 | A `json.Number` label is a float (WP-3-6, deviation 4). See Q-255 and D-228. |
| X-186 | - | The integer kinds of expression values are `int`, `int64`, `time.Month` and |
| X-187 | WP-3-6 | Pointers are not modelled, except `*time.Location` (WP-3-6, deviation 6). Only map |
| X-188 | WP-3-6 | Four multi-value `time.Time` methods are not exposed (WP-3-6, deviation 7). `Date`, |
| X-189 | WP-3-6 | Set constants are typed `map[int]interface {}` (WP-3-6, deviation 8). Go's optimizer |
| X-190 | WP-3-6 | `pow` uses `f64::powf` (WP-3-6, deviation 9). Go's `math.Pow` can differ in the last bit |
| X-191 | WP-3-6 | `toJSON` does not apply the duplicate-name rule (WP-3-6, deviation 10). `encoding/json` |
| X-192 | WP-3-6 | The Go oracle module is a Gorse-patched `expr-lang` (WP-3-6, oracle 1). The module in the |
| O-73 | WP-3-6 | The corpus compares map-order results as multisets (WP-3-6, oracle 2, open). A list |
| O-74 | WP-3-6 | `const_expr` is not run (WP-3-6, oracle 3, closed). Gorse configures no `ConstFns`, and Go |
| Q-257 | WP-3-3 | goptuna's `floats.Argsort` sorts its input in place (WP-3-3, provisional D-WP33-1). goptuna's |
| Q-258 | WP-3-3 | `ModelSearch` takes its models in name order (WP-3-3, provisional Q-WP33-2). Go's |
| X-193 | WP-3-3 | Gonum's `floats.Sum` and `CumSum` are summed in sequence (WP-3-3, provisional D-WP33-2). On |
| X-194 | WP-3-3 | Each sampler owns its random source (WP-3-3, provisional D-WP33-3). goptuna's multinomial and |
| X-195 | WP-3-3 | `Erf`, `Max` and `Min` are ported locally (WP-3-3, X-WP33-3). `math.Erf` is ported from |
| X-196 | WP-3-3 | The `tpe` module headers name goptuna and Go 1.27, not the add21b5 pin (WP-3-3, X-WP33-4). |
| X-197 | WP-3-3 | The tuning hook is `SuggestTrial` in model-core (WP-3-3, X-WP33-5, closes X-65 in a changed form). Go's |
| X-198 | WP-3-3 | goptuna's relative sampling, pruners and storage are not ported (WP-3-3, scope). The relative |
| Q-259 | WP-3-5 | Tags come only from processed labels (WP-3-5, quirk 1). `flatten` in Go's |
| Q-260 | WP-3-5 | Expression failures are logged, not returned (WP-3-5, quirk 2). Go's item embedding |
| Q-261 | WP-3-5 | Errors come in Go's order (WP-3-5, quirk 3). `new_item_to_item` checks the vector store, then |
| Q-262 | WP-3-5 | A query drops the row's own ID and a non-positive dot score (WP-3-5, quirk 4). `QueryItemToItem` |
| Q-263 | WP-3-5 | Scores depend on the type (WP-3-5, quirk 5). The `auto` type halves the store's score |
| X-199 | WP-3-5 | Go sorts the feedback slice in place (WP-3-5, deviation 1). `usersItemToItem.Add` and |
| X-200 | WP-3-5 | The tests pass processed labels (WP-3-5, deviation 2). Go's tests pass raw labels, such as a |
| X-201 | WP-3-5 | An empty result is an empty `Vec`, not `nil` (WP-3-5, deviation 3, open). Go's |
| X-202 | WP-3-5 | `n` is `usize` (WP-3-5, deviation 4). Go's `n` is an `int`. A negative `n` makes Go's `make` in |
| X-203 | WP-3-5 | `IDF[T]` is not ported (WP-3-5, deviation 5). `item_to_item.go` 247 declares a generic `IDF` |
| X-204 | WP-3-5 | `labels_value` is copied three times (WP-3-5, deviation 6). `item_to_item.rs`, |
| X-205 | WP-3-5 | Neighbours come from an exact store (WP-3-5, deviation 7). Go's `xvec` answers with HNSW, which |
| Q-265 | WP-4-2 | Live check of the item-to-item and user-to-user `offset` past the end (WP-4-2, Q-WP42-1, open). The source |
| Q-266 | WP-4-2 | `float_roundtrip` on `serde_json` (WP-4-2, Q-WP42-2, approved, D-243). The workspace `serde_json` enables it i |
| Q-267 | WP-4-2 | A float label keeps its shortest text (WP-4-2, Q-WP42-3; answered, D-266). A label such as `2.50` in a request |
| X-212 | WP-4-2 | TLS to the master is not ported in the server (WP-4-2, deviation 1). `Server::serve` refuses a configured |
| X-213 | WP-4-2 | OpenTelemetry tracing is not applied in `Sync` (WP-4-2, deviation 2). Go's `Sync` sets up the tracer from |
| X-214 | WP-4-2 | `/debug/pprof` and the Swagger UI are not served (WP-4-2, deviation 3). `/apidocs.json` is served from the |
| X-215 | WP-4-2 | go-restful's CORS container filter is not implemented (WP-4-2, deviation 4). Success responses still carry |
| X-216 | WP-4-2 | `RemoteAddr` is always empty in request events (WP-4-2, deviation 5). The router is served without |
| X-217 | WP-4-2 | The REST histogram has a help text (WP-4-2, deviation 6). Go registers an empty help text, which the |
| X-218 | WP-4-2 | Exponent-form floats are written by `serde_json` (WP-4-2, deviation 7). The values are equal. The text of very |
| X-219 | WP-4-2 | JSON member names are matched without regard to case (WP-4-2, deviation 8; fixed, D-244). Go's decoder |
| X-220 | WP-4-2 | Trailing slashes are trimmed before routing (WP-4-2, deviation 9). As go-restful does. `/api/latest/` routes |
| X-221 | WP-4-2 | Numbers in request labels are `json.Number` in Go (WP-4-2, deviation 10). Go decodes request bodies with |
| X-222 | WP-4-2 | Request bytes are the bytes the handler reads (WP-4-2, deviation 11). They are counted by a body wrapper, as |
| X-223 | WP-4-2 | The not-found text includes the name (WP-4-2, deviation 12). `item 6 not found`. Go's storage layer adds the |
| X-224 | WP-4-2 | `serde_json` parses floats exactly (WP-4-2, deviation 13, provisional). Through `float_roundtrip` (Q-266). A |
| X-225 | WP-4-2 | The server name falls back to the `HOSTNAME` variable (WP-4-2, court finding, 2026-10-29; resolved, 5210131). |
| X-238 | WP-4-2 | JSON error texts and the content-type gate (WP-4-2, X-WP42-14; resolved, D-265). The request decoder is a |
| X-239 | WP-4-2 | The host name is read from `/proc` only (WP-4-2, X-WP42-15, provisional, not blocking). Go tries `uname(2)` |
| X-240 | WP-4-2 | Labels are read with serde, so Go's accepted numbers and depths are refused (WP-4-2, X-WP42-16; resolved, D-26 |
| X-241 | WP-4-2 | XML request bodies are not decoded (WP-4-2, X-WP42-17; resolved, D-266). go-restful registers an |
| X-242 | WP-4-2 | Compressed request bodies are not inflated (WP-4-2, X-WP42-18; resolved, D-266). go-restful's |
| X-243 | WP-4-2 | Unassigned code points are quoted as themselves (WP-4-2, X-WP42-19; resolved, D-266). Error texts quote |
| X-244 | WP-4-2 | A Go-valid label can overflow the stack in the storage backends (WP-4-2, X-WP42-20; resolved, D-274). The |
| X-245 | - | A batch of items is written in statements of 1,000 rows (O-87 extra assignment; items resolved, users and feed |
| X-247 | - | Go's recommender orders exact score ties differently from run to run, so the oracle compares tie groups as set |
| X-246 | - | `update_scores` on a fresh index: Go's paged update does not lose keys in any condition tried, so the single-q |
| X-248 | - | The user and feedback batch inserts write in 1,000-row statements (O-87, resolved in 4ed7c45). The same limit  |
| X-249 | - | A sorted search on a fresh RediSearch index can miss a document written just before (O-87, quirk logged; Go sh |
| Q-268 | WP-4-1 | Should `mixed_worker` also run with `collaborative.type = "mf"` (WP-4-1, Q-WP41-1, open). Once Go and Rust |
| X-226 | WP-4-1 | `worker_name` is the hex MD5 of the hostname, `http_host` and the decimal `http_port`, concatenated (WP-4-1, |
| X-227 | WP-4-1 | `pull_users` assigns users by the consistent ring of the live peers (WP-4-1, X-WP41-2). If this node is not |
| X-228 | WP-4-1 | The health error body is `{}` (WP-4-1, X-WP41-3). The Rust `HealthError` is an empty struct that serialises |
| X-229 | WP-4-1 | JSON responses are sniffed as `text/plain; charset=utf-8` (WP-4-1, X-WP41-4). Errors use Go's `http.Error` |
| X-230 | WP-4-1 | The step-seconds gauges are never added to (WP-4-1, X-WP41-5). They stay at zero, as in Go. The |
| X-231 | WP-4-1 | A late tick is skipped (WP-4-1, X-WP41-6). A tick that fired long before it was handled is skipped |
| X-232 | WP-4-1 | A recommend run reports progress every 10 seconds (WP-4-1, X-WP41-7). |
| X-233 | WP-4-1 | A nil and an empty recommendation list write the same cache rows (WP-4-1, X-WP41-8, O-76). The SQL store's |
| X-234 | WP-4-1 | `serve` was not `Send` (WP-4-1, X-WP41-9, fixed in `fe0a19e`). The unit tests never spawned `serve`, so |
| X-235 | WP-4-1 | `Pool` string alignment is dropped (WP-4-1, X-WP41-10). Go's `Pool` string reaches no output, so nothing |
| X-236 | WP-4-1 | The worker connects to the master over mutual TLS (WP-4-1, X-WP41-11, closed by round 2, D-264). Before |
| X-237 | WP-4-1 | The step gauges are carried over (WP-4-1, X-WP41-12). See X-230. |
| O-77 | WP-4-1 | Reranker template output checked against Go (WP-4-1, O-WP41-1, closed by round 2, D-264). |
| O-78 | WP-4-1 | The worker's TLS to the master (WP-4-1, O-WP41-2, closed by round 2, D-264). Wired through |
| O-79 | WP-4-1 | The backend suites need their emulator variables (WP-4-1, O-WP41-3, closed by the gate setup). Without the |
| O-80 | WP-4-1 | The mixed test covers only the latest recommender (WP-4-1, O-WP41-4, open; does not block acceptance, D-264).  |
| O-81 | WP-4-1 | The mixed test has no empty-list case (WP-4-1, O-WP41-5, open). Every user gets 28 rows, so the nil-versus-emp |
| O-82 | WP-4-1 | The mixed comparison is not mutation-checked by the thread (WP-4-1, O-WP41-6). The court runs the mutation |
| O-83 | - | The cross-read tests leave Qdrant collections behind (court finding, D-262, open). `vector_cross_read` writes |
| O-84 | WP-4-1 | Go panics on a template list that mixes bool, string and nil (WP-4-1, round 2, open). `gonja` panics on |
| O-85 | WP-4-2 | The decode oracle is run by hand, and its filter drops rows (WP-4-2; resolved, D-266). `rest::tests::decode` |
| O-86 | WP-4-2 | The mixed test has no XML, compressed or large-number cases (WP-4-2; resolved, D-266). The 63 requests of |
| O-87 | WP-4-2 | Two TEST failures under the parallel run pass alone (WP-4-2 gate, open). On `321946c5` the workspace run faile |
| X-245 | WP-4-3 | (WP-4-3 deviation 9; items resolved by the O-87 thread, see the X-245 entry above). The court's first record:  |
| X-248 | - | (court's first record, superseded by the O-87 entry above) The SQLite batch inserts for users and feedback sti |
| X-249 | - | (court's first record, superseded by the O-87 entry above) A sorted content search can miss a document on a fr |
| O-88 | WP-4-3 | The master does not set the dataset gauges (WP-4-3 deviation 1; resolved, D-276). Go sets `UsersTotal`, `Items |
| O-89 | - | `mixed_cluster_all_rust` timed out in the full gate, and passed alone (closed by D-292, court, 2026-11-01). On |
| O-75 | WP-3-5 | The master must pass processed items to the logics (WP-3-5; resolved, D-275). The Rust master's item and |
| O-76 | WP-3-5 | The REST work must confirm nil against empty results (WP-3-5, open). X-201. Open until the REST |
| Q-264 | WP-3-7 | `recommendItemToItem` and `recommendUserToUser` walk a map (WP-3-7, quirk 1). Go pushes the |
| X-206 | WP-3-7 | A negative cache size maps to 0 (WP-3-7, deviation 1). Go's `make([]T, 0, n)` panics on a |
| X-207 | WP-3-7 | The not-found errors wrap `StorageError::NotFound` (WP-3-7, deviation 2). The texts are |
| X-208 | WP-3-7 | Each Rust test opens its own stores (WP-3-7, deviation 3). Go's suite shares one set of stores |
| X-209 | WP-3-7 | The ported suite does not cover the user-to-user digest for type `items` (WP-3-7, deviation 4). |
| X-210 | WP-3-7 | `lo.Every` takes the collection first (WP-3-7, deviation 5, a bug found and fixed). `lo.Every( |
| X-211 | WP-3-7 | `Recommend` takes the limit as `i64` (WP-3-7, deviation 6). Go's `int`. A limit of 0 or less |
| X-250 | - | The OIDC login sends no `state` and checks no nonce (upstream gap, kept for parity; security note; coordinator |
| X-251 | WP-4-4 | The WP-4-4 interop does not run a Go server and a Go worker beside the Rust master (resolved by D-281; the bri |
| X-252 | - | Go's paged `UpdateScores` lost 66 of 1,000 fresh-index trials once under a busy server, and the port's paged l |
| X-256 | WP-4-8 | The github playground stores 331,901 feedback rows from a dump of 357,294 records (Go parity, WP-4-8, verified |
| X-255 | - | Go's CORS filter copies its start-time lists while its first meta sync may already be writing them (Go quirk,  |
| C-36 | WP-3-8 | The built-in test set shares the training dictionaries, as Go's `LoadDataFromBuiltIn` does (WP-3-8, Go parity; |
| Q-269 | WP-3-8 | Go's parallel BPR is not reproducible (Go quirk, WP-3-8). `SetJobs(NumCPU)` updates the factors without a lock |
| X-253 | WP-3-8 | Parallel BPR on Rust must sit inside Go's ±0.01 band in 12 of 12 automated runs, instead of being compared wi |
| X-254 | WP-4-5 | The master's HTTP listener serves no `/apidocs` page, no `/debug/pprof/` handlers, and no configured CORS; the |
| X-257 | WP-5-2 | WP-5-2 harness and suite entry (D-WP52-1, D-WP52-2, D-WP52-3). The WP-4-7 harness moved to `conformance/tests/ |
| X-258 | WP-5-2 | WP-5-2 SQLite legs (X-WP52-1). The SDK legs use SQLite, so no backend is on their path. The court's 12-backend |
| X-259 | WP-5-3 | WP-5-3 CPU brand line (X-WP53-1). Go's CPU line names the brand and Rust's does not. The test leaves the brand |
| X-260 | WP-5-3 | WP-5-3 JSON log lines before the welcome block (X-WP53-2). Rust's JSON log lines print before the welcome bloc |
| X-261 | WP-5-3 | WP-5-3 CDN stand-in (D-WP53-1). Both binaries hard-code cdn.gorse.io. The test sends the download through `HTT |
| X-262 | WP-5-1 | WP-5-1 score tolerance (X-WP51-1). Scores are compared within 1e-6 of `max(/score/, 1)`, the rule accepted in  |
| X-263 | WP-5-1 | WP-5-1 node version string (X-WP51-2). Accepted (D-308). Go's `Node.Version` is a string that carries the buil |
| X-264 | WP-5-1 | WP-5-1 task count (X-WP51-3). Accepted (D-308). Go's `Progress.Count` is an int with live progress, so it depe |
| C-37 | WP-5-2 | WP-5-2 vendored `gorse_rs` (D-WP52-4). `third_party/gorse-rs` holds gorse_rs 0.5.0 at `gorse-io/gorse-rs` `2de |
| O-90 | WP-5-1 | WP-5-1 blockers B1 to B4 (open). B1 and B2 (dashboard config and category routes: Go 415, Rust 400 and 500) an |
| Q-270 | WP-5-1 | WP-5-1 quirks (Q-WP51-1, Q-WP51-2). Five table rows omit a path placeholder. Go writes no update time for cold |
| O-91 | WP-5-1 | WP-5-1 B3 residual, the reply size in the too-large text (closed by D-299). The Go master's reply size varies  |
| O-92 | WP-4-3 | Label float digits differ from Go (open, owner WP-4-3). On the item-batch and latest routes, 51 batch items an |
| O-93 | WP-4-3 | Dropped -0.0 score on the query reply (open, owner WP-4-3). prost skips 0.0 by value, so the query reply is 5  |
| O-91 | WP-4-3 | closed by D-299. O-92 and O-93 closed by D-301. The B3 residual is governed by D-299 (placeholder for N only). |
| O-90 | WP-4-4 | closed (D-306, D-300, D-299, D-302 to D-304). B1 and B2 are fixed in WP-4-4's `cors.rs` and accepted (D-306).  |

## Table 4: D verdicts that name a deviation

- D-282 O-87 accepted and closed: X-246 closed with no deviation, X-247 approved as a test-oracle tie grouping, X-248 and X-249 
- D-294 The gorse-cli output layers match Go, checked against the Go oracle (WP-4-6; court rejection, coordinator rulings). The 
- D-279 Cross-crate deviation: the external pull keeps a `null` list, and the tasks answer is pinned (WP-4-4 follow-up, O-76; re
- D-296 Court acceptance for WP-5-2 on the integration head `0e209c2` (integrator, 2026-11-02). The branch `claude/wp-5-2-sdk` w
- D-297 Court acceptance for WP-5-3 on the integration head `0e209c2` (integrator, 2026-11-02). The branch `claude/wp-5-3-playgr
