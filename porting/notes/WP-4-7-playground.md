# WP-4-7 playground notes: the playground tests (`crates/gorse-cmd/tests/playground.rs`)

Provisional ids, folded into `PORTING.md` and `PORTING_NOTES.md` by the integrator.
Base: integration head `e9ede16`. Branch `claude/wp-4-7-playground`.

## Tests

| Test | State | Covers |
|---|---|---|
| `playground_ml_100k_restores_from_a_local_dump_and_serves` | runs | The import through `restore_playground`, served from a loopback server: 943 users, 1,682 items, 100,000 feedback, and `/api/users?n=3` gives `1`, `10`, `100`. |
| `playground_ml_100k_binary_writes_its_config` | runs | The real binary's config file and the two lines it prints before its download: `Generated config file:` and `Using cache directory:`. The child is stopped before the download, so no network is needed. |
| `playground_ml_100k_imports_the_dataset_and_serves` | ignored, `O-1: dataset from cdn.gorse.io` | The original binary test, body unchanged: the welcome line after the download. |
| `playground_github_restores_the_dump_and_serves` | runs, unchanged | The GitHub import (from the WP-4-8 thread). |

## Deviations (D-WP47P-n)

1. **The welcome test stays ignored.** The welcome line is printed only after the download, and the binary
   cannot be pointed at a local server. `cmd/gorse-in-one/main.go:151-152` and `crates/gorse-cmd/src/in_one.rs:33`
   hard-code `https://cdn.gorse.io/example/`. Neither reads an override, and no production change was made.
2. **The config check runs the real binary, not the function.** `write_playground_config` is private, and a
   test cannot call it without a production change. The test runs the binary and stops it once it has printed the
   cache directory. It checks the same file and lines without touching the network.
3. **The seam test binds ephemeral master ports.** `Master::serve` binds `master.port`, which defaults to 8086. The
   GitHub test also binds it, and both ran in one process, so the first run failed with `AddrInUse`. The seam test
   sets `master.port = 0` and `master.host = 127.0.0.1`. The GitHub test is unchanged.
4. **`serve_dump` is generalised.** `serve_file(route, body)` serves any route, and `serve_dump` calls it with
   `/github.bin.gz`, so the GitHub test's call site is unchanged.

## Evidence

`cargo test -p gorse-cmd --test playground -- --nocapture` (log `/mnt/project-files/reports/wp-4-7/playground-all.log`):
`test result: ok. 3 passed; 0 failed; 1 ignored`. The ignored test is the welcome test in deviation 1.
`cargo clippy -p gorse-cmd --all-targets -- -D warnings` exits 0; `cargo fmt --all --check` passes.
