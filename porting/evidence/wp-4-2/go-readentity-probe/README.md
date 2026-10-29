# WP-4-2: the Go decode oracle

`main.go` calls go-restful v3.13.0's `ReadEntity` on each body it reads from stdin, as JSON lines with the fields `target`, `ct`, `ce` and `body` (or `hex`), and prints one line per body: Go's error text, or the value Go decoded, or `MARSHAL ERROR` when Go decodes a value it cannot write back. `ReadEntity` runs the Go 1.27.0 `encoding/json` decoder for JSON bodies and `encoding/xml` for XML bodies. The `ce` field sets `Content-Encoding`, so gzip and deflate bodies go through go-restful's own inflation.

`data/data.go` and `server/server.go` are copies of the Go types that the bodies decode into, with their fields: `storage/data`'s `User`, `UserPatch`, `ItemPatch` and `FeedbackKey`, and `server/rest.go`'s `Item` and `Feedback`. Neither has XML tags, so `encoding/xml` matches the Go field names exactly.

`corpus4.jsonl` is the corpus that the probe was first run on, with every row kept.

There is no stored output. The conformance test `conformance/tests/go_decode.rs` runs the probe on `corpus4.jsonl`, on the named XML bodies, and on bodies it generates (every truncation and corruption of the samples, and gzip, zlib and deflate streams of them), then compares each reply with the Rust decoder's. Run it with:

    GOTOOLCHAIN=go1.27.0 cargo test -p gorse-conformance --test go_decode

To run the probe by hand:

    GOTOOLCHAIN=go1.27.0 go run . < corpus4.jsonl
