# O-91 evidence: the master's replies on the golden's item and latest routes

Probe: `conformance/tests/o91_probe.rs` (ignored test `o91_probe_master_replies`). It seeds one Go
cluster and one Rust cluster from the MovieLens 100k dump, as the WP-5-1 golden does, sends the
masters the calls the server makes for the golden's requests (`user-id=1`, `n=5`, `offset=0`),
and writes the raw reply bytes. Run it with:

    O91_OUT=<dir> cargo test -p gorse-conformance --test o91_probe -- --ignored --nocapture

It takes about 40 minutes, since it seeds two clusters.

## The calls

User 1 has feedback on 272 items (`k`). The server (`server/rest.go`) then asks for:

- the item routes: `queryN = max(0 + 5 + k, CacheSize) = 277`, `QueryVectors(top_k = queryN + 1 = 278)`,
  the self item dropped (`logics/item_to_item.go`), then `BatchGetItems` on the first 277 others
  (`FilterVisibleItemsByCategories`);
- `/api/latest`: `GetLatestItems(limit = 0 + 5 + k = 277)`.

## Sizes (bytes), one probe run

| Reply | Go | Rust | Golden N (Go / Rust) |
|---|---|---|---|
| `QueryVectors`, 278 vectors | 1718859 | 1718854 | not in B3 (under 4 MiB) |
| `BatchGetItems`, 277 items (item routes) | 9076124 | 9076124 | 9076002 / 9076124 |
| `GetLatestItems`, 277 items (`/api/latest`) | 9071660 | 9071660 | 9071596 / 9071660 |

The golden's B3 rows are `ResourceExhausted ... (N vs. 4194304)`, where N is the size of the
`BatchGetItems` reply (item routes) or the `GetLatestItems` reply (`/api/latest`).

## What the bytes show

1. **The Go size changes between runs of the same code.** The golden's Go N (9076002, 9071596) is
   122 and 64 bytes below this run's Go N (9076124, 9071660). The Rust N is the same in both runs.
2. **The only size-varying field is `update_at`.** The item and latest replies have the same fields
   and lengths in both masters. `update_at` is the wall-clock time of the restore. In this run,
   Go's nanoseconds lie between 594 and 885 ms into one second, and Rust's between 635 and 834 ms.
   Every nanoseconds value is a 5-byte varint. A restore that starts earlier in the second can put
   some items below 268 ms, where the varint is 4 bytes. That moves the reply size by one byte per
   such item, which is the size of the 122 and 64 byte differences.
3. **Apart from `update_at`, the items match except for float digits.** 226 of the 277 batch
   items and 236 of the 277 latest items are byte-identical outside `update_at`. The other 51
   batch items and 41 latest items differ only in the `labels` field, by one digit of a float64
   at an exact 17-digit tie (for example Go `0.026849746704101562`, Rust `...563`). The lengths
   are equal, and the values are the same float64.
4. **The neighbour reply is 5 bytes smaller in Rust.** Go sends the score of the item itself as
   `-0.0` (fixed32 `0x80000000`). Rust omits it, because prost does not encode a float that equals
   zero. The other neighbours' scores differ only in the last float bits (X-WP51-1). This reply is
   under the 4 MiB limit, so it is not in the B3 rows.

## What this does and does not show

- No payload field is missing or extra in the B3 replies, and no length differs for a reason that
  stays the same between runs.
- The B3 rows' N depends on the sub-second phase of the masters' restores. The Go N is not
  reproducible run to run, so a fixed payload cannot make the two N values equal every time.
- Two real byte differences remain, and neither changes a size on the B3 routes: the `-0.0` score
  field (item 4) and the float tie digits in labels (item 3).

## Open question for the court

Whether the golden's B3 rows may compare N within a bound that the `update_at` widths explain,
instead of exact equality. The coordinator's ruling asked for exact text, and the text contains N.
