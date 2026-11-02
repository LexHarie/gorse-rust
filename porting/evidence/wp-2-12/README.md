# WP-2-12 evidence: the B3 reply sizes (O-91)

This folder holds the byte-level evidence that the court asked for on B3. The
client fix for B3 is in `crates/gorse-storage/src/{data,cache,vectors}/proxy.rs`.
The residual difference is O-91, which the court ruled is in the payload that
the master sends, not in the client.

## What is recorded

`golden-b3-sizes.txt` holds the lines of the WP-5-1 golden run (release) that
show the B3 rows. Each row is one server route whose reply from the master is
over the 4 MiB receive limit. Go's and Rust's errors have the same code, the same
text and the same limit (`4194304`). Only N differs. N is the length prefix of
the gRPC message that the server's client refused, so it is the size of the
protobuf reply that the master sent:

| Route (server node)                              | Go master N | Rust master N | Difference |
|--------------------------------------------------|-------------|---------------|------------|
| `GET /api/item-to-item/{name}/{item-id}`         | 9076002     | 9076124       | 122 bytes  |
| `GET /api/item/{item-id}/neighbors/`             | 9076002     | 9076124       | 122 bytes  |
| `GET /api/item/{item-id}/neighbors/{category}`   | 9076002     | 9076124       | 122 bytes  |
| `GET /api/latest`                                | 9071596     | 9071660       | 64 bytes   |

## What is not recorded

The first differing bytes are not in this folder. The server node's client
drops the reply when it refuses the length, so the bytes of the master's reply
were never seen by the golden test. Capturing them needs a probe that makes the
same gRPC call to each master as the server node makes, with a receive limit
above 9 MB, and writes the raw reply. That probe is the next step for O-91's
owner.

## How the numbers were produced

    cargo test --release -p gorse-conformance --test rest_golden -- --nocapture

The test file is `conformance/tests/rest_golden.rs` at `17184b5` on
`claude/wp-5-1-rest-golden`, copied into a working tree without a commit. The
only local change was to widen the failure excerpt from 80 to 400 characters, so
that the full text, with N, appears in the failure message. The comparison is
unchanged. The run takes about 17 minutes. The run finished with 107 routes
compared, 93 identical, 28 explained and 12 unexplained, and it failed on the 12.
