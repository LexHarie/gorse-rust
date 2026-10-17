# Storage schemas

Everything under `data/`, `cache/`, `meta/` and `samples/` was produced by
`contracts/tools/dump-schemas.sh`: it opens each store with the Go
implementation, runs its own `Init()` (and, for `*.search.*`, `Reconcile()` with
every search column kind), writes one sample of each record kind through the Go
API, and dumps what the database then contains.

Servers used: MySQL 8.0.46, PostgreSQL 10.0, MongoDB 6.0.28, Redis 8.10.2 (RediSearch and
RedisTimeSeries built in), SQLite via `modernc.org/sqlite`. These are the images
in `storage/docker-compose.yml`. All tables were created with an empty table
prefix; with a prefix every table, collection, index-owning table and Redis key
listed here is prefixed (see `storage/scheme.go` `TablePrefix`), but index names
are not.

A Rust store passes the schema contract when a database it initialises dumps
byte-identical to these files (same tool, same server version) and a database
initialised by Go is used by Rust without migration.

## Data store (`data/`)

Backends: MySQL, Postgres, SQLite, MongoDB (`storage/data/`). Tables `users`,
`items`, `feedback`.

| File | Content |
|---|---|
| `data/mysql.sql`, `data/postgres.sql`, `data/sqlite.sql` | `Init()` only |
| `data/*.search.sql` | after `Reconcile` with search columns `item.ItemId`, `item.Categories`, `item.Comment`, `item.Labels`, `item.Labels.title` |
| `data/mongodb.json`, `data/mongodb.search.json` | collections and indexes |
| `samples/data.*.json` | one user, item and feedback as stored |

Notable facts the dumps show (each reproduced, not changed, by the port):

- The three SQL dialects differ on purpose: MySQL uses `utf8mb4_bin` ids,
  `datetime` (whole seconds) for `time_stamp`/`updated` and `datetime(6)` for
  `update_at`; Postgres uses `COLLATE "C"` ids and `timestamptz`; SQLite stores
  times as text and declares `is_hidden` as `numeric`.
- MySQL truncates `time_stamp` to whole seconds; Postgres and SQLite keep
  microseconds; MongoDB keeps milliseconds (`samples/`).
- Default for `users.labels` is `'[]'` on MySQL/Postgres but `"null"` on SQLite.
- Search: MySQL adds a stored generated column `gorse_search_document` and a
  `FULLTEXT` index `gorse_search_index`; Postgres adds a GIN expression index
  `gorse_search_index` over `to_tsvector('simple', ...)`; SQLite adds an FTS5
  virtual table `gorse_search_index` kept in sync by three triggers; MongoDB adds a
  text index `gorse_search_index`. The column expressions are sorted
  lexicographically before being joined, so their order in the dumps is the sort
  order of the SQL fragments, not config order.
- MongoDB field names are the Go field names lower-cased by the BSON codec
  (`userid`, `itemid`, `ishidden`, `updateat`, `feedbackkey.{feedbacktype,userid,itemid}`).
- SQLite queries on categories call a custom scalar function `json_contains(a, b)`
  that the Go process registers on every connection (`storage/scheme.go`); a
  database file does not contain it.

## Cache store (`cache/`)

Backends: Redis (standalone and cluster), MySQL, Postgres, SQLite, MongoDB
(`storage/cache/`). Key formats and value encodings are in `../cache-keys.md`.

| File | Content |
|---|---|
| `cache/mysql.sql`, `cache/postgres.sql`, `cache/sqlite.sql` | tables `values`, `documents`, `time_series_points` |
| `cache/mongodb.json` | collections and indexes |
| `cache/redis.ft-info.txt` | `FT.INFO documents` for the RediSearch index created by `Init()` |
| `samples/cache.*.json` | a value of each encoding, one score document, one time-series point |

Postgres requires the `btree_gin` extension (`Init()` runs `CREATE EXTENSION IF
NOT EXISTS btree_gin`, which needs a role allowed to create extensions). MySQL
creates `idx_collection_subset_categories` with a multi-valued functional index
except on MariaDB, where it is skipped.

## Meta store (`meta/`)

The master keeps cluster metadata in a local SQLite file
`<cache-path>/meta.sqlite3` (`master/master.go`; `storage/meta/sqlite.go`). It is
private to the master process, but a Rust master taking over a Go master's
`--cache-path` reads it, so its schema is frozen too: `meta/sqlite.sql`.

`key_values` holds:

| Key | Value |
|---|---|
| `COLLABORATIVE_FILTERING_MODEL` | JSON of `meta.Model[cf.Score]`: `{"ID": <int64 model id>, "Type": "<model name>", "Params": {<param>: <value>}, "Score": {"NDCG", "Precision", "Recall"}}` |
| `CLICK_THROUGH_RATE_MODEL` | JSON of `meta.Model[ctr.Score]`: same shape, `Score` is `{"RMSE", "Precision", "Recall", "Accuracy", "AUC"}` |
| `RECOMMEND_CONFIG` | JSON of `config.RecommendConfig` (Go field names) saved by `POST /api/dashboard/config`; overrides the file's `[recommend]` section at start |

`nodes` rows expire after `master.meta_timeout`; `ListNodes` deletes expired rows.

## Vector store

Backends: xvec (embedded, default `xvec://<dir>`), Qdrant, Milvus, Weaviate
(`storage/vectors/`). Workers and servers reach an xvec store only through the
master's `VectorStore` gRPC service; they connect to Qdrant, Milvus and Weaviate
directly.

Collection names (`storage/vectors/database.go`):

| Collection | Content |
|---|---|
| `collaborative_filtering_<model id>` | item embeddings of CF model `<model id>` |
| `item_to_item_<name>` | vectors of item-to-item recommender `<name>` |
| `user_to_user_<name>` | vectors of user-to-user recommender `<name>` |

`database.vector_table_prefix` is applied only by xvec (directory name
`<prefix><collection>`); the Qdrant, Milvus and Weaviate constructors ignore it.

Every vector carries `id`, `categories`, `hidden`, `timestamp` and either a dense
FP16/FP32 vector or a sparse vector (dimension 0, dot product only).

| Backend | Layout (source) |
|---|---|
| Qdrant | One named vector `vector`, dense datatype `Float16`, or a sparse vector `vector`; point id = UUIDv3 (MD5, URL namespace) of the Gorse id; payload `id`, `categories` (list), `hidden` (bool), `timestamp` (Unix ms int) (`qdrant.go`) |
| Milvus | Fields `id` VarChar(65535) primary key, `categories` Array<VarChar> capacity 100, `hidden` Bool, `timestamp` Int64 (Unix ms), `vector` Float16Vector(dim) or SparseFloatVector; index on `vector` named `vector` (metric COSINE/L2/IP, IVF variants for quantization) and a sorted index on `timestamp`; description `gorse collection` (`milvus.go`) |
| Weaviate | Class = collection name with the first letter upper-cased; vectorizer `none`; properties `originalId` string, `categories` string[], `hidden` boolean (filterable), `timestamp` date (filterable, range); object id = UUIDv3 (MD5, URL namespace) of the Gorse id; distance `cosine`/`l2-squared`/`dot` (`weaviate.go`) |
| xvec | One directory per collection under the store root, created by `github.com/gorse-io/xvec` with fields `vector`, `categories`, `timestamp`, `hidden` (`xvec.go`). The on-disk format belongs to the xvec library; see `PORTING.md` decision D-6 |

The Go `vectors.Distance` enum is `Cosine = 0`, `Euclidean = 1`, `Dot = 2`; on the
wire (`vector_store.proto`) it is shifted by one (`Unknown = 0`, `Cosine = 1`, ...).
Quantization types `""`,
`sq`, `pq`, `rq` map to backend-specific index or quantization settings in each
adapter.

## Blob store

Configured by `blob.uri`: `s3://bucket/prefix`, `gs://bucket/prefix`,
`az://container/prefix`, or a local directory. A local directory on the master is
exported to workers through the `BlobStore` gRPC service (`storage/blob/blob.go`).

Blob names are model ids in decimal (`strconv.FormatInt(id, 10)`); the ids are
Unix milliseconds taken when training finished. CF and CTR models share the
namespace. Blob contents are specified in `../model-blobs.md`.
