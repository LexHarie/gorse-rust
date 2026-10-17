# Cache key catalogue

Source: `storage/cache/database.go` (names), every `CacheClient` call site in
`master/`, `worker/`, `server/`, `logics/` (usage), and the backends in
`storage/cache/` (encodings). Encodings were confirmed by writing through the Go
stores (`schema/samples/cache.*.json`) and by reading the cache of a running Go
master and server.

The cache store holds three kinds of data: **values** (string key to string
value), **score documents** (sorted, filterable recommendation lists) and **time
series points**.

## Key construction

`cache.Key(parts...)` joins its arguments with `/`, skipping empty parts after
the first: `Key("a", "", "b") == "a/b"`, `Key("a") == "a"`.

The configured cache table prefix (`database.cache_table_prefix`, falling back to
`database.table_prefix`) is applied by the backend, not by `Key`:

| Backend | Values | Score documents | Time series |
|---|---|---|---|
| Redis | string key `<prefix><key>` | hash `<prefix>documents:<collection>:<subset>:<id>`, indexed by RediSearch index `<prefix>documents` (prefix `<prefix>documents:`) | RedisTimeSeries key `<prefix>time_series_points:<name>` |
| MySQL, Postgres, SQLite | table `<prefix>values`, column `name` | table `<prefix>documents` | table `<prefix>time_series_points` |
| MongoDB | collection `<prefix>values`, `_id` = key | collection `<prefix>documents` | collection `<prefix>time_series_points` |

The `documents` and `time_series_points` names are fixed: they come from
`storage.TablePrefix.DocumentTable()` and `PointsTable()`.

## Value encodings

| Constructor | Stored string | Read back with |
|---|---|---|
| `cache.String(name, v)` | `v` | as is |
| `cache.Integer(name, v)` | `strconv.Itoa(v)` | `strconv.Atoi` |
| `cache.Time(name, t)` | `t.String()`, Go's default time format `2006-01-02 15:04:05.999999999 -0700 MST`, **including the monotonic clock suffix** ` m=+39.779705272` when `t` came from `time.Now()`, and the zone abbreviation of the writer's local time zone | `dateparse.ParseAny`, then `.In(time.UTC)` |

Observed in a running Go server's SQLite cache:
`last_modify_user_time/u1 = 2026-10-17 02:58:44.902094534 +0000 UTC m=+39.779705272`.
A value written from a fixed time (not `time.Now()`) has no `m=` suffix:
`2026-01-02 03:04:05.678901 +0000 UTC`.

A missing key reads as the zero value (`""`, `0`, zero `time.Time`) with no error
(`ReturnValue`).

## Value keys

`<x>` is a path component filled at run time.

| Key | Encoding | Written by | Read by |
|---|---|---|---|
| `global_meta/num_users` | Integer | master `tasks.go` (load dataset) | master `GET /api/dashboard/stats` |
| `global_meta/num_items` | Integer | master | master stats |
| `global_meta/num_total_pos_feedbacks` | Integer | master | master stats |
| `global_meta/num_user_labels` | Integer | master | master stats |
| `global_meta/num_item_labels` | Integer | master | master stats |
| `global_meta/num_valid_pos_feedbacks` | Integer | master | master stats |
| `global_meta/num_valid_neg_feedbacks` | Integer | master | master stats |
| `global_meta/last_fit_matching_model_time` | Time | master after training the CF model | master stats |
| `global_meta/last_fit_ranking_model_time` | Time | master after training the CTR model | master stats |
| `global_meta/last_update_latest_items_time` | Time | (constant defined; read only) | master stats |
| `global_meta/last_update_popular_items_time` | Time | (constant defined; read only) | master stats |
| `last_modify_user_time/<user_id>` | Time | server on user insert/modify and feedback insert | worker (skip inactive users), master dashboard |
| `last_modify_item_time/<item_id>` | Time | server on item insert/modify and feedback insert | — |
| `non-personalized_update_time/<name>` | Time | master after computing recommender `<name>` | server, master: `Last-Modified` header |
| `non-personalized_digest/<name>` | String (MD5 hex of name, score, filter) | master | `logics` (staleness check) |
| `recommend_update_time/<user_id>` | Time | worker | worker, server `Last-Modified`, master dashboard |
| `recommend_digest/<user_id>` | String (`RecommendConfig.Hash()`) | worker | worker (staleness check) |
| `collaborative-filtering_update_time/<user_id>` | Time | worker | — |
| `collaborative-filtering_digest/<user_id>` | String (`CollaborativeConfig.Hash()`) | worker | `logics` |

Constants that exist in `storage/cache/database.go` but have no cache call site at
the pinned commit: `item-to-item_digest`, `item-to-item_update_time`,
`user-to-user_digest`, `user-to-user_update_time` (item-to-item and user-to-user
neighbours now live in the vector store, see `schema/README.md`).

## Score document collections

A score document is `(collection, subset, id, score, is_hidden, categories,
timestamp)`. `SearchScores(collection, subset, categories, begin, end)` returns
non-hidden documents of one subset containing **all** given categories, sorted by
score descending; `end = -1` means up to 10 000 (Redis) / unbounded (SQL, Mongo).

| Collection | Subset | Ids | Written by | Read by |
|---|---|---|---|---|
| `non-personalized` | recommender name | item ids | master | server, `logics`, dashboard |
| `recommend` | user id | item ids | worker | server, worker, dashboard |
| `collaborative-filtering` | user id | item ids | worker | `logics` |
| `item_categories` | `""` | category names | master | master `GET /api/dashboard/categories` |

`cache.ItemCache = [non-personalized, item-to-item, recommend]` is the list of
collections the server patches when an item is hidden, re-categorised or deleted.

The empty category `""` acts as "match everything": reads without a category
search for `[""]` (`server.ReadCategories` default), and the server adds `""` to
every document it patches (`server/rest.go` `withWildCard`). Which writers add
`""` is decided per writer (`logics/non_personalized.go`, `worker/pipeline.go`)
and is ported with them, not specified here.

### Per-backend encoding of a score document

| Backend | `categories` | `is_hidden` | `timestamp` | `score` |
|---|---|---|---|---|
| Redis hash | each category base64 (std alphabet, no padding) of `"_" + category`, joined with `;` — `["", "c1"]` is `Xw;X2Mx` | `0`/`1` | Unix microseconds | Go `%v` float |
| SQLite | JSON text `["","c1"]` | `0`/`1` | `2026-01-02T03:04:05.678901Z` text | real |
| MySQL | JSON text, functional index `CAST(categories AS CHAR(255) ARRAY)` (skipped on MariaDB) | tinyint | `datetime(3)` | double |
| Postgres | `text[]` `{"",c1}`, GIN index with `btree_gin` | boolean | timestamptz | numeric |
| MongoDB | array | bool | BSON date (milliseconds) | double |

Redis query strings escape `- : . / +` with a backslash (`escape` in
`storage/cache/redis.go`).

## Time series

`AddTimeSeriesPoints` / `GetTimeSeriesPoints(name, begin, end, bucket)`. Redis
uses `TS.ADD ... DUPLICATE_POLICY LAST` with millisecond timestamps and
`TS.RANGE ... AGGREGATION last <bucket>`; the SQL and Mongo backends emulate the
same "last value per bucket" read.

| Name | Written by |
|---|---|
| `num_users`, `num_items`, `num_feedback`, `num_pos_feedbacks`, `num_neg_feedbacks` | master when it loads the dataset |
| `cf_ndcg`, `cf_precision`, `cf_recall` | master after CF training |
| `ctr_precision`, `ctr_recall`, `ctr_auc` | master after CTR training |
| `positive_feedback_ratio` and `positive_feedback_ratio_<feedback_type>` | master `metrics.go` |

The dashboard reads them through `GET /api/dashboard/timeseries/{name}`.
