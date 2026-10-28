# WP-1-15 YAML notes (provisional ids, folded into PORTING.md / PORTING_NOTES.md at merge)

Scope: viper's YAML codec, `yaml.Unmarshal(b, &m)` into `map[string]any`,
over go.yaml.in/yaml/v3 v3.0.4 (add21b5). Code: `crates/gorse-config/src/formats/yaml*`.
Entry point: `gorse_config::formats::yaml::decode(input: &[u8]) -> Result<Map, String>`.

## Decisions

- D-WP115Y-1 (replaced the adapter): the WIP adapter over yaml-rust2
  (e5f9b7d) is gone. The port follows yaml.v3's pipeline stage by stage:
  readerc.go, scannerc.go, parserc.go, the composer, decode.go and resolve.go.
  The adapter gave 518 golden mismatches (error texts, marks, the depth-255
  limit that yaml.v3 does not have). yaml-rust2 is not in the workspace or the
  lock. No crate-selection row named it, so none needs removing.
- D-WP115Y-2 (value model): `decode` returns `gorse_config::value::Map`, the
  model the TOML and dotenv decoders and the viper and mapstructure drafts
  already use. The coordinator's note asked for `gorse_common::gojson::Value`.
  The integrator should confirm which one the config loader takes.
- D-WP115Y-3 (deep nesting): the conversion from decoded Go values to `Value`
  (`formats/yaml.rs`, `value()`) grows the stack with `stacker::maybe_grow`,
  as the composer does. Without it, a 9990-deep document overflowed a 2 MiB
  thread. stacker is the crate-selection row's choice (0.1.25, latest). The
  row lists go-toml; the YAML path now uses it too, so the row should say so.
- D-WP115Y-4 (test thread): `golden_cases` replays on a 512 MiB thread.
  Depth-10000 cases overflow a default test thread in debug builds.

## Deviations

- X-WP115Y-1 (non-UTF-8 bytes): a Go string is bytes. Only a `!!binary`
  scalar, or a percent-escape in a tag, can make it non-UTF-8. Rust `String`
  cannot hold those bytes, so each run of invalid bytes becomes one U+FFFD
  (`lossy_runs`). Tags use `from_utf8_lossy`. In the golden, 33 cases differed
  only in their `hex` and `k_hex` fields, which give the raw bytes. The replay
  therefore compares the lossy text and does not compare those two fields.
  Checked by stripping `hex` from the Go outcome and comparing the rest.

## Quirks reproduced

- Q-WP115Y-1: an unmarshal error for a `!!map` or `!!seq` tag includes the raw
  value text, as yaml.v3's `terror` does (`decoder.rs`). Four golden error
  texts depend on it.
- Q-WP115Y-2: a latched scanner error is reported on parser entry and again
  after the state machine, which is yaml.v3's `peek` behaviour (`parser.rs`).
- Q-WP115Y-3: a reader error is raised only when the scanner needs characters
  past the decoded window. Decoding keeps the characters before that window
  (`reader.rs`, `scanner.rs` `ensure`).
- Q-WP115Y-4: a scanner error message gets one extra line, as yaml.v3's
  `fail()` gives it.
- Q-WP115Y-5: comment marks decide where BLOCK-END tokens fall
  (`scanner.rs` `unroll_indent`). Ported as in `scannerc.go`.

## Open

- O-WP115Y-1 (needs integrator sign-off): dropping a 10000-deep
  `gorse_config::value::Value` overflows a 2 MiB stack in a debug build.
  Decoding such a document is safe. The drop is still recursive. The likely
  fix is an iterative `Drop` for `Value`, which touches the shared
  `value.rs`.
- O-WP115Y-2: yaml.v3 compacts its token queue when the backing array is
  full. `scan_comments` looks at `tokens[len-2]`, so in rare cases Go could
  see a different token. Not emulated. No golden case has shown it.
- O-WP115Y-3: `is_bom` checks the BOM at the start of the decode window
  (`buffer_start`). yaml.v3's `is_bom` reads `buffer[0..3]` and ignores its
  index argument, so the two agree only when the window starts at the stream
  start. No golden case differs.
- O-WP115Y-4: the config golden (`config_golden.json`, `configgolden`) needs
  `LoadConfig` and the config core. Neither is on this branch, so its YAML
  cases stay ignored until that lands.

## Evidence

- `yaml_golden.json`: 11435 cases. `cargo test -p gorse-config --test yaml_golden`
  passes. `YAML_GOLDEN_REPORT` writes an empty report, so 0 cases differ
  under the X-WP115Y-1 comparison.
- `cargo test -p gorse-config`: all suites pass (dotenv goldens 3, dotenv
  gotenv 11, toml deep 6 with 1 ignored, toml golden 1, toml unit 24, unit 38).
- `cargo fmt --all --check`: exit 0.
- Workspace `clippy -D warnings` and `cargo test --workspace`: pass on this tree.
- Toolchain: rustc and cargo 1.99.1. Every locked crate in the YAML path
  matches crates.io's latest version (stacker 0.1.25, regex 1.13.1,
  serde_json 1.0.154, sha2 0.11.1, hex 0.4.3).
