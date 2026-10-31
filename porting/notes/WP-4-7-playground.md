# WP-4-7 playground notes: the ml-100k playground test

Provisional ids, folded into `PORTING.md` and `PORTING_NOTES.md` by the integrator.
Base: integration head `e9ede16`. Branch `claude/wp-4-7-playground`.

## Deviations (D-WP47P-n)

1. **The ml-100k playground test runs in process, not as a child binary (`crates/gorse-cmd/tests/playground.rs`).**
   The old test ran `gorse-in-one --playground=ml-100k`, which fetches `PLAYGROUND_URL` (the CDN, blocked by O-1).
   The binary takes no base URL, so it cannot read a local server. The test now follows the GitHub test: it
   serves `/mnt/project-files/datasets/ml-100k.bin.gz` at `/ml-100k.bin.gz` from a loopback server and imports
   it through `restore_playground`, the function the playground uses.
2. **The binary-level checks are no longer run by any test.** The old test asserted the `Welcome to Gorse
   Playground` line and the written `~/.gorse/etc/config.toml`. Those come from `import_playground` and
   `write_playground_config`, which need the CDN. They are not covered here. The integrator should decide whether
   to keep a test for them.
3. **The ml-100k test binds ephemeral gRPC and HTTP ports.** `Master::serve` binds `master.port`, which defaults to
   8086. The GitHub test binds that port too, and both run in one process, so the first run failed with
   `AddrInUse`. The ml-100k test sets `master.port = 0` and `master.host = 127.0.0.1`. The GitHub test is unchanged.
4. **The `serve_dump` helper is generalised.** `serve_file(route, body)` serves any route; `serve_dump` calls it
   with `/github.bin.gz`, so the GitHub test's call site is unchanged.
5. **Expectations.** The ml-100k test checks 943 users, 1,682 items and 100,000 feedback, the counts that
   `client/setup-test.sh` restores, and the first three users from `GET /api/users?n=3` are `1`, `10`, `100`,
   the order that `TestUsers` in `client/client_test.go` expects. These are not changed to match the output.

## Evidence

`cargo test -p gorse-cmd --test playground -- --nocapture` (log `/mnt/project-files/reports/wp-4-7/playground-all.log`):
`playground_ml_100k_imports_the_dataset_and_serves ... ok` and `playground_github_restores_the_dump_and_serves ... ok`,
`test result: ok. 2 passed; 0 failed; 0 ignored`.
`cargo clippy -p gorse-cmd --all-targets -- -D warnings` exits 0; `cargo fmt --all --check` passes.
