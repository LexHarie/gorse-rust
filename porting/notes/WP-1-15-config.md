# WP-1-15 config: the `config.go` port

Go sources: `config/config.go` and `config/config_test.go` at add21b5, with
`config/config.toml` (identical to `contracts/config/config.toml`).
Rust target: `crates/gorse-config`, module `config` (`config.rs` and
`config/{schema,defaults,load,validate,json}.rs`).
Branch `claude/project-thread-d6aixe`, merged with integration `c4f9c58`.

Ids below are provisional. The next free ids in `PORTING_NOTES.md` were Q-145,
C-27, D-82, X-46 and O-17 at the time of writing. The integrator should fold
these entries into `PORTING_NOTES.md` and renumber if another thread took one.

## Status

- Ported: the `Config` tree and its mapstructure decoding, defaults and
  `set_default`, `LoadConfig` through viper, `Validate`, the recommender hashes,
  and the `Meta.config` JSON (`json.Marshal` and `json.Unmarshal`).
- Acceptance (`cargo test -p gorse-config`) passes. The
  `contracts/config/meta-config.default.json` and
  `meta-config.sample.json` contracts are reproduced exactly (test
  `meta_config_matches_contracts`).
- Not yet: the YAML cases (X-47) and the log calls (X-48).

## Quirks reproduced

- **Q-145 Duplicate keys that differ only by case.** `config.toml` with
  `Port = 1111`, `PORT = 2222` and `port = 3333` under `[master]`. Go's viper
  walks its key map in random order, so the golden file records two outcomes:
  `master.port` 1111 or 2222. The port gives 2222, the second recorded outcome,
  because `viper::insensitivise` keeps the last spelling other than the
  lower-case one (`insensitivise_keeps_the_last_other_spelling`). The port's
  choice is pinned by `pinned_alternatives`, which asserts it exactly.
- **Q-146 A file value shadowing an env-bound key.** `master = 5` in the file
  and `GORSE_MASTER_PORT=4` in the environment. Go's `AllKeys` order is random,
  and viper's `deepSearch` replaces an intermediate scalar with a map. If
  `master` is walked after `master.port`, the scalar wins and the error is
  `decoding failed ... 'master' expected a map or struct, got "int64"`. If it is
  walked first, the nested map wins and Validate reports
  `n_jobs must be greater than 0`. The port walks keys in sorted order, so the
  nested map wins and the result is the second recorded outcome. Pinned by
  `pinned_alternatives`. The rule that a scalar on the path becomes a map is
  how `viper.rs` models viper's `deepSearch`. The golden file shows both
  outcomes, which is the evidence that the order matters.
- **Keys are lower-cased** the way viper lower-cases them (Q-144, Go's case
  mapping).
- **`json.Marshal` of a NaN or infinite float** fails with
  `json: unsupported value: NaN` (or `+Inf`, `-Inf`). The port returns the same
  text from `marshal_json` (golden `meta_error`).
- **Go's `encodeFloat`** writes `e-09` as `e-9` and uses exponent form only for
  magnitudes below 1e-6 or at least 1e21.
- **Go's JSON string escaping** (`<`, `>`, `&` and the control characters) is
  reused from `gorse_common::expression::go_json_quote`.
- **Durations** are integer nanoseconds in the JSON form and Go duration strings
  in TOML.

## Deviations

- **D-82 Validate is explicit ordered checks.** The Go validator tags
  (`gte=0`, `gt=0`, `oneof=`, and the `item_expr` and store checks) are coded
  as functions in `config/validate.rs`, run in struct field order, and the first
  failure wins. The messages match the golden file
  (`test_validate_messages_match_the_golden`, 36 validate cases). The
  go-playground validator is not a dependency.
- **D-83 Injected environment.** `load_config_with_env(path, lookup)` takes the
  environment lookup. `load_config(path)` reads the process environment. The
  crate forbids `unsafe`, and edition 2024 makes `std::env::set_var` unsafe, so
  the tests do not mutate the environment.
- **D-84 Logging is a placeholder** (`src/log.rs`, `TODO(WP-1-8)`). Go's
  `LoadConfig` warns when the file is missing, and the port does not print that
  warning yet. The util thread's API is `tracing` directly:
  `tracing::warn!("message")`, with no wrapper. Swap the placeholder when WP-1-8
  merges (X-48).
- **D-85 `go_json_quote` is public** in `gorse-common/src/expression.rs`. It was
  private. Config's JSON writer needs Go's string escaping.
- **D-86 `md-5` and `hex` are direct dependencies.** `gorse_common::util::md5`
  sorts its input, but Go's `md5.New()` hashes stream the parts in order
  (`NonPersonalized`, `External`, `ItemToItem`, `UserToUser` and `Collaborative`
  hashes). `md5_of_sorted` uses `util::md5` only for `RecommendConfig::hash`.
- **D-87 `serde_json` is a normal dependency** (it was a dev-dependency). It
  parses `Meta.config` in `config/json.rs`.
- **D-88 `TracingConfig::equal` takes `&TracingConfig`.** Go's nil receiver
  returns false, and a Rust reference cannot be nil.
- **D-89 JSON decode stops at the first type mismatch.** Go's `json.Unmarshal`
  records the first mismatch and keeps decoding, so fields after it are still
  set in Go. The port returns the first mismatch at once. No golden case reaches
  this, and the inputs Gorse decodes are its own `json.Marshal` output.
- **D-90 The TOML `TestUnmarshal` case runs from the text.** Go re-encodes the
  text through `yj` before decoding. The port decodes the text directly. The
  result is the same.

## Open

- **X-46 `NewTracerProvider` is not ported.** It builds the OpenTelemetry
  exporter from `TracingConfig`. No work package in `porting/work-packages.md`
  owns tracing. See O-17.
- **X-47 YAML cases are ignored**, with `#[ignore = "needs WP-1-15 yaml"]`:
  `config::tests::test_unmarshal_yaml` and the golden `load_yaml_cases`. The
  YAML decoder is `formats::yaml::decode(&[u8]) -> Result<Map, String>`, already
  dispatched at `viper.rs:146`. Un-ignore both when the YAML branch merges.
- **X-48 Log output is missing** until WP-1-8 merges (D-84).
- **X-49 `contracts/config/config.schema.json` and `config.keys.md` are not
  generated** by this port. They come from `contracts/tools/configschema`
  (`jsonschema` reflection and the walk over `GetDefaultConfig`). WP-1-15's
  acceptance names only `meta-config.*.json` and `config.toml`.
- **O-17 Who owns `NewTracerProvider`?** For the integrator to assign (X-46).

## Verification

- `cargo test -p gorse-config`: unit 61 passed, 1 ignored (YAML); the
  `config_golden` binary 6 passed, 1 ignored (YAML load cases). The golden binary
  passed on five repeated runs.
- `cargo test --workspace`: 677 passed, 0 failed, 9 ignored. Two of the ignored
  are the YAML cases above; the other seven were already ignored on the
  integration branch.
- `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` both exit 0.

The golden harness sets `HOME` in the environment map it hands to the loader,
as `contracts/tools/configgolden` does with `os.Setenv("HOME", ...)`, so a
dotenv `$HOME` expands the same way in both.
