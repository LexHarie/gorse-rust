# WP-1-15 part T: viper's TOML codec (go-toml v2.2.4)

Ids are provisional (`PORTING.md` D-26); the main thread assigns the final
numbers when it merges.

## Scope

`gorse_config::formats::toml::decode(input) -> Result<Map, String>` is what
spf13/viper v1.21.0's TOML codec (`internal/encoding/toml/codec.go`,
`toml.Unmarshal(b, &v)` with `v map[string]any`) leaves in the map, or the
error's text (viper adds `While parsing config: `). `unmarshal` returns the
same result with the error typed (`Error::Decode` for a `*toml.DecodeError`,
with its line, column and human-readable context; `Error::Other` for errors
go-toml makes with `fmt.Errorf`).

Ported by hand from github.com/pelletier/go-toml/v2 v2.2.4 (MIT, licence in
`crates/gorse-config/licenses/GO-TOML-LICENSE`):

| Rust module | Go source |
|---|---|
| `formats/toml/parser.rs` | `unstable/parser.go`, `unstable/scanner.go` |
| `formats/toml/ast.rs` | `unstable/ast.go`, `unstable/builder.go`, `unstable/kind.go` |
| `formats/toml/characters.rs` | `internal/characters/ascii.go`, `internal/characters/utf8.go` |
| `formats/toml/decode.rs` | `decode.go` (and the `LocalTime` precision of `localtime.go`) |
| `formats/toml/tracker.rs` | `internal/tracker/seen.go` |
| `formats/toml/unmarshaler.rs` | `unmarshaler.go`, the paths reachable from a `map[string]interface{}` root |
| `formats/toml/errors.rs` | `errors.go` (`DecodeError`, `wrapDecodeError`), `unstable/parser.go` `ParserError`, `internal/danger/danger.go` `SubsliceOffset` |

Go types map to `crate::value::Value` as the module doc of `formats/toml.rs`
lists (`int64` is `Int64`, `time.Time` is `Time` in UTC or
`time.FixedZone("", offset)`, `toml.LocalDate`/`LocalTime`/`LocalDateTime`
keep their `Precision`). `value.rs` is used as it is. The branch carries
one commit from `claude/wp-1-15-config`, cherry-picked unchanged (`gorse-config:
fix a doc link to Value::kind`), because without it
`RUSTDOCFLAGS="-D warnings" cargo doc -p gorse-config` fails on `value.rs`.

## Evidence

- `crates/gorse-config/tests/toml_golden.rs` against
  `crates/gorse-config/tests/data/toml_golden.json`: 10,970 documents, every
  one the same as Go (map with Go types, float bits, time zones; or error
  text, `DecodeError` position and human context byte for byte), except the
  four listed for X-WP115T-2. Go results: 4,166 maps, 6,641
  `*toml.DecodeError`s, 163 other errors, no panics.
  Corpus (deduplicated by bytes): 2,342 string literals of go-toml's own
  `*_test.go` files (all of them, `toml_testgen_test.go` included) plus its
  fuzz corpus and benchmark documents; 257 further toml-test documents
  (BurntSushi/toml v1.6.0 `internal/toml-test/tests`, TOML 1.0 and 1.1, and
  the TOML blocks of `specs/v1.0.0.md`); Gorse's `config/config.toml` and the
  358 `config.toml` texts of `config_golden.json`; 902 edge cases
  (`contracts/tools/tomlgolden/cases.go`); 7,110 seeded single-byte mutations.
- Go coverage of the corpus (`go build -cover` of the golden tool):
  `unstable` 94.6 %, `internal/tracker` 91.9 %, `internal/characters` 100 %,
  `decode.go` all but the two `panic`s. Not covered: `KeepComments` and
  `Shape` (unused by `Unmarshal`), `kind.go` `String` (only in panics),
  `seen.go` panics for impossible node kinds, `key.go` (strict mode), and in
  `unmarshaler.go` the struct, pointer, typed-map, array and
  `TextUnmarshaler` branches, which a `map[string]interface{}` root never
  reaches.
- `crates/gorse-config/tests/toml_unit.rs` (24 tests) and unit tests in
  `parser.rs`, `errors.rs`: go-toml's own tests, ported (list below).
- `crates/gorse-config/tests/toml_deep.rs`: documents 100,000 levels deep
  (arrays, inline tables, arrays of inline tables, dotted keys, table
  headers, errors) on the default test stack; and X-WP115T-1.

Regenerate the golden (needs github.com/BurntSushi/toml v1.6.0 in the module
cache: `go mod download github.com/BurntSushi/toml@v1.6.0`; go.mod is not
changed):

```
go run ./contracts/tools/tomlgolden -out crates/gorse-config/tests/data/toml_golden.json
```

Two runs give byte-identical files.

## Quirks reproduced

- **Q-WP115T-1 One expression at a time.** `Unmarshal` parses one top-level
  expression, checks its keys and decodes it before it parses the next
  (`unmarshaler.go:251-260` `fromParser`, `unstable/parser.go:99`
  `NextExpression`). An error in expression n (a key defined twice, an
  integer out of range, an impossible date) wins over a syntax error after
  it, even on the same line: `a=1\na=2 x` is `toml: key a is already
  defined`, `a=2021-13-01 x` is `toml: impossible date`, `a=1\n=\na=2` is
  the syntax error. Evidence: golden `edge order 0` to `14`.
- **Q-WP115T-2 Swapped arguments and a double space** in
  `internal/tracker/seen.go:268`: `toml: key %s already exists as a %s,  but
  should be an array table` gets the kind first and the key second
  (`toml: key value already exists as a fruit,  but should be an array
  table`). Evidence: golden `go-toml toml_testgen_test.go:114`, `:1629`,
  `:1644`.
- **Q-WP115T-3 Message texts kept verbatim**, grammar included: `toml: key a
  should be a table, not a array table` (`seen.go:213`), `hour cannot be
  greater 23`, `seconds cannot be greater 60` (`decode.go`), `couldn't parse
  octal number: strconv.ParseInt: parsing "1000000000000000000000": value out
  of range` (strconv's text inside). Evidence: golden `edge order 9`, `edge
  integer 63`, `TestUnmarshalDecodeErrors` port.
- **Q-WP115T-4 `nan`, `+nan` and `-nan` are all `math.NaN()`**, bits
  `0x7FF8000000000001`; the sign of `-nan` is dropped (`decode.go:285`).
  Evidence: golden `edge keyword 18` to `20`, unit test `nan_is_gos`.
- **Q-WP115T-5 A leading zero at the very end of the document.** The `0`
  prefix check in `scanIntOrFloat` needs more than two bytes
  (`unstable/parser.go:1125`): `a=01` is `toml: leading zero not allowed on
  decimal number` (1:3), `a=01\n` is `toml: expected newline but got U+0031
  '1'` (1:4), `a=0x` is `expected newline but got U+0078 'x'`. Evidence:
  golden `edge integer 6`, `7`, `36`.
- **Q-WP115T-6 The space of a date-time skips a byte.** `scanDateTime` adds 2
  at a space followed by a digit and the loop adds 1
  (`unstable/parser.go:1083`), so the byte after the first digit of the time
  is never looked at: `a=1979-05-27 0x:32:00` is `toml: expected digit (0-9)`
  at 1:15, not a newline error. Evidence: golden `edge datetime 65`.
- **Q-WP115T-7 Two messages for a control character in a string.** A string
  without escapes is validated in one pass and reports `invalid UTF-8`
  (`unstable/parser.go:711`, `:904`); once it has an escape, the slow path
  reports `invalid character U+0001` (`:797`, `:957`). Evidence: golden
  `edge basic string 33`/`34`, `edge multiline basic string 36`/`37`.
- **Q-WP115T-8 Fractional seconds past nine digits are cut, not rounded**,
  and the precision stays 9 (`decode.go:253-262`): `00:00:00.1111111119` is
  111111111 ns. Evidence: `TestIssue710` port, golden.
- **Q-WP115T-9 `\e` is accepted** (TOML 1.1, `unstable/parser.go:773`,
  `:932`). Evidence: golden `toml-test tests/valid/string/escape-esc.toml`,
  `config golden toml 1.1 escape e`.
- **Q-WP115T-10 `%c` of a byte** prints the byte as the rune of the same
  number, so byte 0xEF is `ï` (`unstable/parser.go:867`, `:1237`, `:1241`);
  `%#U` quotes the rune only when `strconv.IsPrint` says so (`U+0020 ' '`,
  `U+00A0` unquoted). Evidence: golden (`invalid character at start of key:
  ï` and others from mutations), unit test `sharp_u_matches_go`.
- **Q-WP115T-11 A value that starts with `}` is an `incomplete number`**:
  `parseVal` hands anything it does not recognise to the number scanner
  (`unstable/parser.go:1198`), so `a={a=}` fails there. Evidence: golden
  `edge nesting 2`, `3`.
- **Q-WP115T-12 Key redefinition follows go-toml's tracker**
  (`internal/tracker/seen.go`): a table header marks its parents explicit
  (`setExplicitFlag`, `:144`), re-entering an array table forgets the keys of
  the previous element (`clear`, `:101`, `:270`), each inline table has its
  own tracker, and `[foo.bar.baz]`, `[foo]`, `[foo.bar]` is accepted
  (issue 739). Evidence: golden, `TestUnmarshal` and `TestIssue*` ports.
- **Q-WP115T-13 Where an error at the end of the document is reported
  depends on the input's spare capacity.** Go reslices `b[len(b):]` to zero
  capacity without moving the pointer when `len == cap`, and
  `danger.SubsliceOffset` (`internal/danger/danger.go:11`) then measures
  from the start of the slice being scanned. With `len == cap`, `a=[` is
  reported at 1:3, `a = "abc` at 1:5, `x=1\ny=[1,` at 2:5; with spare
  capacity at 1:4, 1:9 and 2:6. viper always has spare capacity: it reads
  the file into a `bytes.Buffer` (`viper.go:1707`), whose `ReadFrom` leaves
  at least 512 free bytes. The port gives the spare-capacity result
  (D-WP115T-5); the golden tool decodes through `bytes.Buffer.ReadFrom` as
  viper does and checks the spare capacity.
- **Q-WP115T-14 Go's stack limit.** Go's goroutine stacks grow to 1 GB; at
  about 828,000 nested arrays go-toml's `parseVal`/`parseValArray`
  recursion (`unstable/parser.go:355`, `:586`) passes it and the process dies
  with `fatal error: stack overflow` (827,734 levels decode, 828,613 crash;
  measured with go1.27.0 on linux/amd64). See X-WP115T-1.

## Deviations

- **X-WP115T-1 No stack limit.** Where Go dies (Q-WP115T-14), the port
  decodes: its recursion runs on a stack that `stacker` grows on demand
  (D-WP115T-4). Mirroring Go would mean aborting the process at a depth that
  depends on Go's frame sizes. Test: `toml_deep::deeper_than_go_stack_limit`
  decodes 1,000,000 nested arrays. It needs about 5 GB and 20 s in a debug
  build, so it is `#[ignore]`d with that reason and run on request:
  `cargo test -p gorse-config --release --test toml_deep -- --ignored`
  (1.9 s, 1.8 GB peak; passes).
- **X-WP115T-2 Floats with more than 800 digits before the point
  (inherited from `gorse_common::gostrconv`, WP-1-4).** When the fast path
  cannot round a number with a non-zero digit past the 800th, Go's
  `strconv.ParseFloat` falls back to its 800-digit `decimal`, whose `set`
  puts the decimal point after the 800th stored digit instead of after the
  last digit read (`$GOROOT/src/internal/strconv/atof.go:110` and
  `:131-133`, with the cap at `:119-123`), so the value is off by a power of
  ten: `9007199254740993` + 790 zeros + `1e-791` is `9.007199254740993e+08`
  in Go, `9007199254740994` (the correctly rounded value) in
  `gostrconv::parse_float`, which hands the exact digits to Rust's parser
  (`crates/gorse-common/src/gostrconv/atof.rs` `convert`). Same in go1.24.7.
  Test: `PARSE_FLOAT_DEVIATIONS` in `toml_golden.rs` lists the four golden
  cases (`edge long float 3` and three mutations of the long-float cases)
  with Go's and Rust's bits and fails if either changes. The fix belongs in
  `gostrconv` (outside this part); see O-WP115T-2.

## Decisions

- **D-WP115T-1 Only the `map[string]interface{}` path of the decoder.** The
  reflect-based decoder is ported as `Target::Interface` (an `interface{}`
  slot: a map entry, a slice element) and `Target::Concrete` (a value of
  concrete type: `map[string]interface{}`, `[]interface{}`), which is all a
  map root reaches. Not ported: structs, pointers, typed maps and slices,
  arrays, `encoding.TextUnmarshaler`, `unstable.Unmarshaler`, strict mode
  (`DisallowUnknownFields`, `internal/tracker/key.go`, `DecodeError.Key`,
  which only strict mode sets: `errors.go:168` from `strict.go:58`, `:70`),
  `KeepComments` comment nodes, `Parser.Shape`, the `LocalDate`/`LocalTime`/
  `LocalDateTime` methods (`AsTime`, `String`, `MarshalText`,
  `UnmarshalText`), `Decoder`/`NewDecoder` (viper calls `Unmarshal`), and
  the marshaler.
- **D-WP115T-2 Unreachable branches stay as panics.** Where go-toml panics on
  an impossible state (`decode.go:22` invalid base, `:93` missing timezone,
  `seen.go:170` node kind, `unmarshaler.go:299`, `:554`, `:707`) or would hit
  a reflect panic on a path a map root cannot take, the port panics with
  Go's text. The golden records Go panics and has none.
- **D-WP115T-3 Strings are UTF-8.** Keys and string values become Rust
  `String`s with `from_utf8_lossy`. go-toml validates every key and string as
  UTF-8 before it stores it, so nothing is ever replaced: the
  golden writes non-UTF-8 Go strings as hex and has none (10,970 cases,
  7,110 of them with random bytes).
- **D-WP115T-4 Recursion on a growing stack.** `parse_val`, the tracker's
  `check_array`, `check_inline_table`, `set_explicit_flag` and `clear`, and
  the decoder's `handle_value`, `handle_key_part` and
  `handle_key_value_part` run inside `stacker::maybe_grow` (128 KiB red
  zone, 4 MiB segments), as `gorse-expr` does; the parser keeps Go's
  recursive structure. On an error, the partly decoded map is dropped
  iteratively (`drop_deep`). The golden test runs on the default test
  stack, 10,000-level documents included. `stacker` (already a workspace
  dependency) is added to `gorse-config`; its `crate-selection.md` row says
  so.
- **D-WP115T-5 The input is taken to have spare capacity** (Q-WP115T-13),
  as viper's always does; `unmarshal`'s doc says so.
- **D-WP115T-6 Keys keep document order.** `Map` is ordered by first
  insertion; Go's map order is random, so nothing can depend on it.
- **D-WP115T-7 Golden encoding.** Each map is a flat pre-order list of nodes
  (`k`, Go type name `t`, value `v`, child count `n`), so neither Go's
  `encoding/json` nor `serde_json` meets its nesting limit on 10,000-level
  documents; floats are their 16 hex digits of `math.Float64bits`; a
  `time.Time` has its zone name, offset and location name; non-UTF-8 text is
  hex.

## Plan corrections

- **C-WP115T-1** `porting/crate-selection.md` names the `toml` crate (1.1.8)
  for viper's TOML. The port does not use it: go-toml's accepted syntax,
  error texts and positions, `\e`, `int64`/`LocalDate` types and the quirks
  above differ from any other TOML parser, and viper's errors reach the user
  (`config.LoadConfig` returns them, `config/config.go:826`). That row is not
  edited here (not this part's file).
- **C-WP115T-2** `porting/work-packages.md` lists only Gorse's
  `config/config.go` for WP-1-15; part T ports third-party code (go-toml
  v2.2.4) under `crates/gorse-config/src/formats/toml/`.

## Open questions

- **O-WP115T-1 Dropping very deep results.** The compiler's drop of a
  `Value` recurses once per level, so a caller that drops a successfully
  decoded document nested tens of thousands of levels deep can overflow a
  2 MiB thread (Go's garbage collector does not recurse). viper's merging
  and mapstructure's decoding recurse too. The viper part of WP-1-15 should
  decide whether `Value` (shared `value.rs`) gets an iterative `Drop`; the
  tests here drop iteratively.
- **O-WP115T-2 `gostrconv::parse_float` and Go's 800-digit decimal**
  (X-WP115T-2): WP-1-4 could reproduce `decimal.set`'s point placement so
  the deviation disappears; `PARSE_FLOAT_DEVIATIONS` then fails and is
  removed.

## go-toml tests ported

Ported (map or `interface{}` targets; adapted targets say so in the test):

- `unstable/parser_test.go`: `TestParser_AST_Numbers`, `TestParser_AST`,
  `TestParser_AST_DateTimes`, `ExampleParser` (`parser.rs` tests).
- `errors_test.go`: `TestDecodeError` (all 8), `TestDecodeError_Accessors`
  (without `Key`), `ExampleDecodeError`.
- `internal/danger/danger_test.go`: `TestSubsliceOffsetValid`,
  `TestSubsliceOffsetInvalid`.
- `unmarshaler_test.go`: `TestUnmarshal_Integers` and `TestUnmarshal_Floats`
  (into a map; `0` is `int64(0)` there, a `float64` field converts it),
  every `TestUnmarshal` example whose target is or holds a
  `map[string]interface{}` (except the one below),
  `TestUnmarshalDecodeErrors` (all), `TestASCIIControlCharacters`,
  `TestLocalDateTime` (expected fields written out instead of
  `time.Parse`), `TestIssue287`, `458`, `507`, `579`, `581`, `585`, `586`,
  `588`, `596`, `600`, `602`, `631`, `658`, `662`, `666`, `701`, `703`,
  `708` and `710` (into `interface{}` maps), `714`, `715`.
- `fast_test.go`: `TestFastSimpleInt`, `TestFastSimpleString` (into
  `interface{}` maps), `TestFastSimpleInterface`,
  `TestFastMultipartKeyInterface`, `TestFastArrayTable`.
- `internal/imported_tests/unmarshal_imported_test.go`:
  `TestUnmarshalEmptyInterface`, `TestInterface`,
  `TestUnmarshalTabInStringAndQuotedKey`, `TestUnmarshalMap`,
  `TestUnmarshalToNilInterface`, `TestUnmarshalMixedTypeSlice` (the
  `interface{}` fields' values, as a map).

Not ported:

- `toml_testgen_test.go` (toml-test, valid and invalid): every input is a
  golden case, and the golden asserts Go's exact result, which is stricter
  than toml-test's typed JSON.
- `TestUnmarshal` "unmarshal key into map with existing value" and
  `TestFastExistingMap`: viper always decodes into an empty map.
- `TestFastSimpleFloat`, `TestUnmarshalOverflows`, `TestUnmarshalFloat32`,
  `TestIssue850`, `TestIssue851` and every other test with a struct, pointer,
  typed-map, array or `TextUnmarshaler`/`Unmarshaler` target
  (`TestUnmarshalErrors`, `TestUnmarshal*InvalidStructField`,
  `TestIssue252`, `484`, `494`, `508`, `564`, `575`, `623`, `677`, `772`,
  `799`, `807`, `866`, `915`, `931*`, `TestUnmarshal_RecursiveTable*`,
  `TestUnmarshal_Nil`, `TestUnmarshal_CustomUnmarshaler`,
  `TestUnmarshalEmbedNonString`, `TestOmitEmpty`, `TestUnmarshalTags`, most
  of `imported_tests`): those decoder paths are not ported (D-WP115T-1).
  Their inputs are golden cases all the same.
- `TestDecoderStrict` (both files), `ExampleDecoder_DisallowUnknownFields`:
  strict mode.
- `TestDecodeReaderError`, `TestUnmarshalInvalidTarget`,
  `TestUnmarshalNonPointer`: no reader or target to get wrong.
- `TestUnmarshalEmptyInterfaceDeep`, `TestGithubIssue437`: skipped in Go
  (`t.Skipf`).
- `localtime_test.go`: the `LocalDate`/`LocalTime`/`LocalDateTime` methods
  are not ported.
- `internal/tracker/seen_test.go` `TestEntrySize` (Go struct size),
  `danger_test.go` `TestStride`, `TestBytesRange` (not used on this path),
  `ExampleParser_comments` (`KeepComments`, `Shape`), benchmarks, fuzz
  (its corpus is in the golden), `marshaler_test.go`.

## Mutation checks

Single-point changes to the Rust code, one at a time, each with the crate's
tests under a timeout and the file restored in a `finally` block (all files
byte-identical afterwards). All 16 were caught; see the commit message for
the list and the tests that caught each.
