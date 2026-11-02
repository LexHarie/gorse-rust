# WP-2-12 notes: the gRPC oversized-reply error (B3)

Branch `claude/wp-2-12-grpc-errors`, based on the integration head `e671168`.

## Status

- **B3, the error mapping: fixed.** tonic's `OutOfRange` text for an oversized
  reply is reported as Go's `ResourceExhausted` with `grpc: received message
  larger than max (N vs. M)`. The data, cache and vector proxy clients all go
  through `rpc_error`, so all three report it. The limit `M` is 4194304 on the
  server node, as in Go. The worker's proxy clients take Go's 512 MiB worker limit.
- **O-91, the residual N: open, not in the client.** The golden run's N differs
  by 122 bytes on the item routes and 64 bytes on `/api/latest`. N is the size of
  the master's reply, so the difference is in the payload the masters send. The
  evidence is in `porting/evidence/wp-2-12/`, and that folder's README says what
  was not captured.

## Decisions

1. The mapping reads N and M from tonic's own text (`Error, decoded message
   length too large: found N bytes, the limit is: M bytes`). Those are the same
   numbers Go's `recvMsg` prints: the length prefix and the limit. tonic 0.14.6
   is pinned in `Cargo.lock`, so the text does not change under the build.
2. The worker's limit is set on its three proxy clients, not on the shared
   channel. The channel is shared with the master client, which already has the
   512 MiB limit, and the setter keeps each client's limit in its own type.

## Evidence

`porting/evidence/wp-2-12/golden-b3-sizes.txt` and `porting/evidence/wp-2-12/README.md`.
