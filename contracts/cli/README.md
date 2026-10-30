# Command-line contract

`*.help.txt` is the `--help` output of the five Go binaries built at the pinned
commit (`go build ./cmd/gorse-*`). `gorse-bench` also has one file per
subcommand that takes flags (`gorse-bench-<subcommand>.help.txt`), since its
subcommands carry the benchmark flags. The only edit is that the build machine's home
directory in the `--cache-path` default is written as `${HOME}`. The Rust
binaries accept the same flag names, short forms, value types and defaults, so
existing service files, container commands and Helm charts keep working.

Behaviour the help text does not show (from `cmd/*/main.go`):

- `-v`/`--version` prints the version banner and exits.
- `gorse-master` and `gorse-in-one` default `--cache-path` to
  `$HOME/.gorse/var/lib/master` through `config.MkDir`, which **creates that
  directory while the flags are being defined**, so even `--help` creates it. A
  non-default `--cache-path` is not created, and the master exits with `failed to
  connect meta database ... unable to open database file (14)` if it is missing
  (observed; `PORTING_NOTES.md`).
- `gorse-server` and `gorse-worker` use TLS towards the master only when all three
  of `--ssl-ca`, `--ssl-cert`, `--ssl-key` are set; any other non-empty subset is
  fatal (`incomplete SSL configuration`).
- `gorse-in-one --playground[=name]` (bare flag means `default`) always writes the
  embedded config to `$HOME/.gorse/etc/config.toml`, overwriting any existing file
  (`ml-100k` uses the client test config, anything else `config/config.toml`),
  prints `Generated config file: <path>`, then downloads `ml-100k.bin.gz` or
  `github.bin.gz` from `https://cdn.gorse.io/example/` and restores it as a
  `/api/dump` stream before running a master, server and worker in one process.
- The config file path comes from `-c`/`--config`; environment variables override
  file values for the keys listed in `../config/config.keys.md`.
