# WP-4-2: the Go decode oracle

`main.go` calls go-restful v3.13.0's `ReadEntity` (the Go 1.27.0 `encoding/json` decoder underneath it) on each body of `corpus4.jsonl`, with the Go type of the route the body is for, and prints Go's error text or decoded value per body. `oracle4.jsonl` is its output at the time of WP-4-2.

`data/data.go` and `server/server.go` are copies of the JSON-relevant fields of `storage/data/database.go`'s `User`, `UserPatch`, `ItemPatch` and `FeedbackKey`, and of `server/rest.go`'s `Item` and `Feedback`.

Run it with:

    GOTOOLCHAIN=go1.27.0 go run . < corpus4.jsonl > oracle4.jsonl

`make_fixture.py` turns the corpus and the oracle into the Rust fixture, `crates/gorse-server/src/rest/tests/go_decode.jsonl`, which `rest::tests::decode` reads. The filters are in the script, with the reason for each.
