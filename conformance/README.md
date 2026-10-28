# Conformance

Differential tests that put the same input through the Go build (the oracle,
built from the Go tree in this repository at the pinned commit) and the Rust
build, and compare the outputs. They decide the exit gates of Phases 2 to 5.

| Test file | Work package | Compares |
|---|---|---|
| `tests/schema.rs` | WP-2-13 | Databases initialised by the Rust stores, dumped with `contracts/tools/schemadump`, against `contracts/schema` |
| `tests/proxy_wire.rs` | WP-2-12 | The proxy stores over gRPC: Go's proxy clients against the Rust servers, and the Rust clients against Go's servers, on one 66-step scenario and the golden transcript in `crates/gorse-storage/tests/data/proxy_transcript.txt` |
| `tests/cf_golden.rs` | WP-3-1 | The CF replay data in `crates/gorse-model/testdata/cf`: `contracts/tools/modelreplay` writes it again with Go, and the output must equal the committed `train.tsv`, `test.tsv` and `golden.json` |
| `tests/metrics.rs` | WP-3-8 | NDCG, precision, recall and AUC of the Rust and Go models on the same dataset files |
| `tests/mixed_cluster.rs` | WP-4-7 | The Go SDK suite (`client/client_test.go`) against Go master + Rust workers, + Rust servers, then all Rust |
| `tests/rest_golden.rs` | WP-5-1 | Every REST route in `contracts/rest/*.routes.md`: status, headers that matter, and body against recorded Go responses |
| `tests/sdk.rs` | WP-5-2 | The `gorse-go` and `gorse_rs` SDK suites against both builds |
| `tests/playground.rs` | WP-5-3 | `gorse-in-one --playground` end to end |
| `tests/vector_cross_read.rs` | WP-2-9 | The Rust and Go vector stores write and read each other's dataset on Qdrant, Milvus and Weaviate. The Go side is `contracts/tools/vectorxread`. Needs Go and the three servers, so the tests are `#[ignore]`d; run them with `-- --ignored` |

Rules:

- The Go side always runs from this tree (`go build ./cmd/...`), never from a
  downloaded release.
- Outputs that the contracts mark as run-dependent (wall-clock times, generated
  ids, map order) are normalised by name, and each normalisation rule is listed in
  the test that applies it. Nothing else is normalised.
- A diff is either fixed or explained by an entry in `PORTING_NOTES.md`; the exit
  gate is zero unexplained diffs.
- Backends come from `storage/docker-compose.yml`. A test that needs a backend or
  a Go binary that is unavailable fails; it is never skipped silently.
- Tests that need a backend are `#[ignore]`d, so that `cargo test` stays offline. Run
  them with `-- --ignored`; once run, a missing backend or Go binary fails them.
