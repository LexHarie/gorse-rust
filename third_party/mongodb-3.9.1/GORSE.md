# mongodb 3.9.1, patched for Gorse

This is the `mongodb` 3.9.1 package as published on crates.io, with two
changes. The workspace uses it through `[patch.crates-io]` in the root
`Cargo.toml`. The change was ruled by the coordinator on 2026-10-29 for X-244
(the Mongo label depth), and it reverses the earlier ruling against forking
the driver. The patch as a diff is `third_party/mongodb-3.9.1.patch`, and it
applies with `patch -p1` to the published package.

## The changes

1. `src/operation/update.rs`: the update operation sends its statements, the
   `updates` array, as an OP_MSG document sequence (payload type 1) with
   identifier `updates`, instead of in the command body. Go's driver does the
   same (`x/mongo/driver/operation.go`, mongo-driver v1.17.9, the version in
   `go.mod`). The change uses the existing `Command::add_document_sequence`.
2. `src/coll/options.rs`: `UpdateModifications::Raw(RawDocumentBuf)`, and
   `From<RawDocumentBuf>` for it. The update document is passed as the raw
   BSON the Gorse writer makes. The `Document` route converts labels through
   the bson crate's tree, which recurses, and that overflowed the stack at
   10,000 levels (D-268).

## Why

The server names an element of a refused update from the start of the command
it receives. With the statements in the command body, the element is named
`updates.0.u.$set…`. With a document sequence, the statement is the top-level
document and the element is named `u.$set…`, as Go's driver names it. The body
also counts two more levels of depth, so MongoDB refuses at a different label
level (D-271, the court's bodies at 321946c5). The sequence fixes both.

## Dropping this patch

Remove the `mongodb` line from `[patch.crates-io]` and this directory, once an
upstream release sends update statements as a document sequence and accepts
raw update documents. Until then, the `porting/notes/WP-4-2.md` deviation says
the same.

## Checks

- `cargo check -p gorse-storage --features mongodb` builds with the patch.
- The live conformance test `mixed_mongo_deep_labels_match_go_server`, run with
  `MONGO_URI`, is the oracle for the wire shape. It must pass on the merged
  head.
