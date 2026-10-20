# WP-1-15 part E notes: the dotenv decoder

Provisional ids; the integrator renumbers them into `PORTING_NOTES.md` and
`PORTING.md`. viper v1.21.0 decodes a config file whose extension is `env` or
`dotenv` with `internal/encoding/dotenv.Codec.Decode`: the bytes go into a
`bytes.Buffer`, `gotenv.StrictParse` (github.com/subosito/gotenv v1.6.0) parses
it, and every variable becomes a string in the map. The port:

- `crates/gorse-config/src/formats/dotenv.rs`: `decode` (the codec),
  `strict_parse` and `Env` (gotenv's `StrictParse` on byte strings),
  `parseLine`, `checkFormat`, `parseExport`, `varReplacement` and the patterns.
- `src/formats/dotenv/scan.rs`: `bufio.Scanner` (go1.27.0) with gotenv's
  `splitLines`, and the reader chain `strictParse` builds (`bytes.Buffer`, the
  three-byte BOM probe through `io.TeeReader`, `io.MultiReader`).
- `src/formats/dotenv/transform.rs`: golang.org/x/text v0.36.0
  `transform.Reader` with the `UTF8BOM` and `UTF16(…, ExpectBOM)` decoders.
- `src/formats/dotenv/gostr.rs`: `utf8.DecodeRune`/`DecodeLastRune`,
  `unicode.IsSpace`, `strings.TrimSpace`, `Index`, `LastIndex`, `ReplaceAll`,
  `SplitN(…, 2)` on bytes.
- Licences: `crates/gorse-config/licenses/GOTENV-LICENSE` (MIT),
  `VIPER-LICENSE` (MIT), `GO-LICENSE` (Go's BSD-3-Clause; x/text ships the
  identical text, checked with `diff`).

The Go oracle is `contracts/tools/dotenvgolden`, which writes
`crates/gorse-config/tests/data/dotenv_golden.json` (8793 cases). Regenerate
from the repository root with:

```
go run ./contracts/tools/dotenvgolden \
  -config-golden crates/gorse-config/tests/data/config_golden.json \
  -gotenv-dir "$(go env GOMODCACHE)/github.com/subosito/gotenv@v1.6.0" \
  -out crates/gorse-config/tests/data/dotenv_golden.json
```

The tool reads no clock and clears the process environment first; two
regenerations are byte-identical (sha256 `9f3fc175…a1273b6d455`).

## Plan corrections

- **C-WP115E-1 gotenv's `TestScanner` does not pass on go1.27.0.** Three of its
  seven cases expect a final empty line after a trailing terminator
  (`"aa\nbb\ncc\n"` → `aa, bb, cc, ""`). Since Go 1.22, `Scanner.Scan` returns
  `token != nil` for `ErrFinalToken`, and `splitLines` returns a nil token at
  EOF, so Go 1.27 gives three lines. Evidence: gotenv v1.6.0's tests run with
  `$GOROOT/bin/go test` (go1.27.0, x/text v0.36.0, testify v1.11.1 from the
  module cache, in a scratch copy): `--- FAIL: TestScanner … expected: 4
  actual: 3` for "regular LF/CR/CRLF split with trailing …"; every other gotenv
  test passes. The port (`scan.rs`, `gotenv_test_scanner`) asserts Go 1.27's
  lines, Q-WP115E-1.

## Decisions

- **D-WP115E-1 Go strings are bytes.** The parser works on `&[u8]` end to end.
  `strict_parse` returns gotenv's `(Env, error)`: an `Env` of byte strings in
  the order keys were first set (a `HashMap` index for lookups) and the
  error's text as bytes; the variables set before an error come back with it,
  which is what `gotenv.Parse` returns and what the ported `TestParse` checks.
  `decode` is the codec: it drops them on error, as viper does, and turns the
  bytes into Rust strings (X-WP115E-1). Keys always match `[0-9A-Za-z_.]+`, so
  they are ASCII and converting them never merges two keys.
- **D-WP115E-2 The read sizes are ported, not just the line rules.** Where a
  line ends depends on what the scanner has buffered (Q-WP115E-2), so the
  scanner keeps Go's buffer (nil, then 4096 doubling to 65536, shifting when
  `start > 0` and the buffer is full or more than half consumed) and every
  reader in front of it returns what Go's returns: `bytes.Buffer.Read` copies
  `min(len(p), remaining)`, the probe's ≤3 bytes come back alone on the first
  read, `transform.Reader` has 4096-byte source and destination buffers.
- **D-WP115E-3 The patterns are Go's, spelled out for the regex crate.** Go's
  `\s` is `[\t\n\f\r ]` and `\w` is `[0-9A-Za-z_]`; the regex crate's are
  Unicode, and its ASCII `\s` includes `\v`, which Go's does not. So the Rust
  patterns write the classes out, run on bytes with Unicode off (`(?-u)`), and
  `\'`, `\"`, `\#` are plain characters. In Go, `.` and the negated classes
  match any rune, with each invalid byte read as U+FFFD; every class in these
  patterns either contains all non-ASCII runes or none, so matching bytes gives
  the same spans. Both engines choose the leftmost-first match and its groups
  the way a backtracking engine would. A unit test derives the Rust
  `linePattern` from the Go text by those substitutions
  (`go_patterns_spelled_out`), and the golden checks the behaviour.
- **D-WP115E-4 Only `StrictParse`'s path is ported.** `override` is always false
  (`Load`, `OverLoad`, `Apply`, `OverApply` set process variables; viper never
  calls them), so `varReplacement` reads the environment first. Its `hsq`
  parameter is kept although the caller always passes false, as in Go.
  `Marshal`, `Write` and viper's `Encode` are not part of decoding.
- **D-WP115E-5 Guards that cannot fire are left out.** The probe's
  `err != io.EOF` check (a `bytes.Buffer` cannot fail) and the scanner's
  `ErrBadReadCount`, `ErrNegativeAdvance`, `ErrAdvanceTooFar` and its panic on
  100 empty tokens (the readers never return more than asked; `splitLines`
  always advances when it returns a token). `ErrNoProgress` and every
  `transform.Reader` error branch are ported, with Go's texts, although the
  decoders here never reach them (the BOM is always there when a UTF-16
  decoder runs, and no sequence needs more than 4 bytes of look-ahead).
- **D-WP115E-6 `env` is `os.LookupEnv`.** `decode` and `strict_parse` take
  `&dyn Fn(&str) -> Option<String>`; tests pass the case's environment. Names
  are `[A-Z0-9_]+`, so they are ASCII.
- **D-WP115E-7 Golden format and corpus.** One case per line; each names an
  environment from the file's `environments` (variables set and unset; the
  tool clears the environment, so nothing else is visible). A byte string is
  a JSON string when it is valid UTF-8 and at most 512 bytes, `{"text", "hex"}`
  when it is not UTF-8, and `{"len", "rle"}` (hex units repeated) when longer.
  Every error is also checked against `viper.ReadConfig`, which must say
  `While parsing config: ` and the same text. The corpus: gotenv's 42
  `formats`, 2 `errorFormats`, `TestMarshal`'s text and its 10 files (57);
  the 9 dotenv files of `config_golden.json`, with `HOME` set and unset (18);
  201 branch inputs with and without an environment (402); 2728 generated
  lines (every value with every separator, every prefix with every key, 1500
  random lines and 600 random files); 69 byte order mark cases; 27 invalid
  UTF-8 inputs; 1286 boundary cases (a break at line offset `2^k·4096 − 1 + δ`
  for δ in −3..2 after five different prefixes, through the plain reader and
  through x/text for UTF-8+BOM, UTF-16LE and UTF-16BE at multiples of 2048,
  wide characters and invalid UTF-8 that overflow x/text's buffers, and 60
  files of random line lengths); 206 lines of 4094 to 70000 bytes; 4000
  seeded mutations (insert, delete, replace; 92% of the written bytes from the
  characters gotenv cares about). 3684 cases parse, 5109 fail.
- **D-WP115E-8 gotenv's files.** `crates/gorse-config/tests/data/gotenv/` holds
  copies of gotenv v1.6.0's `.env`, `.env.invalid` and `fixtures/*` for the
  ported tests (`diff -r` against the module: identical). The golden tool
  embeds them as Go literals and `-gotenv-dir` checks them byte for byte.
- **D-WP115E-9 Dependencies.** `regex` (already a workspace dependency, row
  "regexp (RE2)" in `porting/crate-selection.md`) for gorse-config;
  `serde_json` and `hex` (workspace dependencies) as dev-dependencies for the
  golden test. No new crate.

## Quirks reproduced

Each is in the golden; `tests/dotenv_golden.rs` (`quirks_recorded_by_go`)
names cases for Q-WP115E-2 to 11.

- **Q-WP115E-1 No empty final line.** A trailing `\n`, `\r` or `\r\n` gives no
  last empty line on go1.27.0 (C-WP115E-1). In `strictParse` an empty line is
  skipped, and inside an open quote it cannot close it, so this changes no
  result; it is visible only to gotenv's own scanner test.
- **Q-WP115E-2 A `\r` at the end of the buffered data ends the line.**
  `splitLines` (gotenv.go) returns `data[:idx]` when the `\r` is the last byte
  it was given, without waiting to see a `\n`; the `\n` then reads as an empty
  line, which a multi-line quoted value keeps: `A="xxxx\r\ny"` gives
  `xxxx\n\ny` when the `\r` lands on the last byte of the scanner's buffer.
  Where that happens: the first read is the probe's 3 bytes (`ab\r\ncd`
  scans as `ab`, ``, `cd`); after the first lines, the partial line is shifted
  to the front and the buffer refilled, so the break hits at line start + 4096
  (then 8192, …, 65536 as the buffer doubles); through x/text, UTF-16 ASCII
  text arrives 2048 decoded bytes per read and UTF-8+BOM text 4096. 96 golden
  cases show it (`boundary/prefix 0 quoted crlf 4096+0` →
  `x×4092 + "\n\ny"`, `…4096-1` and `…4096+1` → one `\n`; also 8192, 16384,
  32768, 65536, `utf16le … 2048+0`, `utf8 … 4096+0`, 24 random files).
- **Q-WP115E-3 The opening-quote check indexes the wrong string.**
  `idx = strings.LastIndex(strings.TrimSpace(val[1:]), quote); if idx >= 0 &&
  val[idx] != '\\'` (gotenv.go `strictParse`): `idx` is into the trimmed
  `val[1:]` but is used on `val`. With no space after the opening quote it
  tests the byte before the last quote; with spaces it tests some other byte.
  `A=" x\"` closes on its line (`A` = ` x\`), `A="  x\"⏎y"` closes on line 1
  and then fails on `y"`, and `A="\t\""⏎B=1` stays open to "missing quotes".
- **Q-WP115E-4 A backslash before the last quote of a line keeps the value
  open,** for single quotes too: `A='x\ny\$FOO\\'` → "missing quotes";
  `A='x\'⏎y'` → `x\'⏎y`. Continuation lines test only the last quote of the
  line and the byte before it.
- **Q-WP115E-5 `StrictParse` rejects every `export NAME` line.** `export B`
  never matches `linePattern` (a separator is required), so `checkFormat` runs
  `parseExport`: the line starts with `export` (`HasPrefix`, so `exporter B`
  counts), it is split at the first space only, and if the rest is not a key
  set earlier in the file (the environment is not consulted) the error is
  "line `…` has an unset variable"; otherwise it is "doesn't match format".
  `export  B` looks up ` B`; `export\tB` has no space and only gets "doesn't
  match format". gotenv's `TestParse` row "allows export line" passes only
  because `Parse` drops the error.
- **Q-WP115E-6 Double-quoted escapes.** `\n` and `\r` are replaced first, then
  `\X` becomes `X` for any `X` but `$`, then `\$` is left to the expansion. So
  `"\\n"` is a line break, `"\t"` is `t`, `"\\$FOO"` is `$FOO` and `"\é"` is
  `é`. Unquoted values keep their backslashes except before `$`.
- **Q-WP115E-7 Expansion.** `variablePattern` takes the braces as optional on
  each side, so `${FOO` and `$FOO}` expand; names are `[A-Z0-9_]+` (`$1`,
  `$FOO_` expand, `$foo`, `${a.b}`, `${}` and `$` stay). The process
  environment wins even when the variable is set to "", then keys set earlier
  in the file (`A=1⏎A=${A}2` reads the environment's `A` if set), else "".
  Expanded text is not expanded again.
- **Q-WP115E-8 A too-long line inside a quoted value is "missing quotes".**
  The continuation loop ends when `Scan` fails and `quote` is still set, so
  `bufio.ErrTooLong` is never reported there. Elsewhere it is
  "bufio.Scanner: token too long". A line must fit with its terminator in
  64 KiB (65535 bytes and `\n` fit; 65536 do not, also at EOF where the
  buffer is full before the reader says EOF); a value of many short lines may
  be longer.
- **Q-WP115E-9 x/text's decoders.** UTF-16: a lone surrogate is U+FFFD per
  unit, but a trail surrogate followed by another trail surrogate is one
  U+FFFD for both (`isHighSurrogate` tests DC00–DFFF); an odd last byte is
  U+FFFD; a second BOM is kept as U+FEFF, which is not white space, so the
  line fails. UTF-8 with a BOM: each maximal ill-formed subpart is one U+FFFD
  (W3C), unlike Go's one per byte, so a file with a BOM never yields invalid
  UTF-8.
- **Q-WP115E-10 `\r` alone ends a line,** inside quotes too (`A="x\ry\r"` →
  `x\ny\n`); `\r\n` inside quotes becomes `\n`.
- **Q-WP115E-11 Separators.** `=` takes white space on both sides; `:` needs at
  least one white space after it and none before (`A:1`, `A :1` fail). The
  multi-line check looks for `=` before `:`, so `A: "x=y` is the unquoted
  value `"x=y`. `A==1` is `=1`. Leading and trailing Unicode spaces of a line
  are trimmed, but `\v` or U+00A0 between the key and `=` fails, while after
  the `=` `strings.TrimSpace` removes them.
- **Q-WP115E-12 Comments and trailing text.** `#` starts a comment anywhere in
  an unquoted value (`A=x#y` → `x`, `A=#x` → ""); a quoted value followed by
  text on its line is read whole as an unquoted value (`A="x" y` →
  `"x" y`); `A="x" "y"` → `x" "y`; across lines the same input fails.
- **Q-WP115E-13 Invalid UTF-8 is a non-space rune.** `TrimSpace` stops at an
  invalid byte; the negated classes match it; error texts carry the bytes.

## Deviations

- **X-WP115E-1 Bytes that are not UTF-8 in a value or an error text.** Go
  keeps them in its strings; `decode` returns Rust strings, where each byte
  that does not start a valid sequence becomes one U+FFFD, as Go's
  `for _, r := range s` reads it. `strict_parse` returns Go's exact bytes for
  every golden case. Only files without a BOM can produce such bytes
  (Q-WP115E-9). Test: `decode_matches_go_except_listed_invalid_utf8` lists the
  31 named cases (`bom/fe fe`, `bom/ff ff`, `bom/fffe after newline`,
  `bom/not a bom fe`, `bom/not a bom ff`, `bom/utf8 partial bom`,
  `bom/utf8 partial bom 1` and 24 `invalid/…` cases) and 582 mutation cases
  by number, checks that exactly those have non-UTF-8 output in Go, and that
  `decode` gives Go's text with the U+FFFD replacement for them and Go's text
  exactly for every other case.

## Open questions

- **O-WP115E-1 `cargo doc` fails on the base commit.** `src/value.rs` (49e7b77,
  shared) links ``[`Value::kind_name`]``, which does not exist, so
  `RUSTDOCFLAGS="-D warnings" cargo doc -p gorse-config --no-deps` stops with
  `unresolved link`. Not edited here (other workers share the file). With the
  link changed to `Value::kind` locally the crate's docs, private items
  included, build without warnings.
- **O-WP115E-2 Environment values that are not UTF-8.** Go's `os.LookupEnv`
  returns any bytes; the `env` callback returns `Option<String>`, so the
  caller (the `LoadConfig` port) decides what such a variable becomes
  (`std::env::var` reports it as not Unicode). The decoder uses whatever the
  callback returns byte for byte.
- **O-WP115E-3 Merge points.** `crates/gorse-config/Cargo.toml` (adds `regex`
  and the dev-dependencies `hex`, `serde_json`), `Cargo.lock` and
  `src/formats.rs` (`pub mod dotenv;`) are also touched by the TOML and YAML
  parts; `licenses/GO-LICENSE` and `VIPER-LICENSE` are verbatim copies, so
  identical additions merge cleanly.

## Tests

Ported (`crates/gorse-config/tests/dotenv_gotenv.rs`, `src/formats/dotenv/scan.rs`):

- gotenv `TestParse` (all 42 rows, `Parse` = the variables `strict_parse`
  returns, with `FOO=test` for the preset row), `TestStrictParse` (both rows,
  error text and variables), `TestStrictParse_NoTokenPassThroughErrors` (the
  endless reader of zero bytes becomes 64 KiB + 1 zero bytes: the same
  `bufio.ErrTooLong`), `TestRead` (the four fixtures and keys), the parsing
  half of `TestLoad_default`/`TestMust_Load_default` (`.env`),
  `TestLoad_overrideVars` and `TestLoad_overrideVars2` (`$A`/`$C` read from
  the environment), `TestLoad_Env` (`.env.invalid` fails),
  `TestLoad_BOM_UTF8`, `TestLoad_BOM_UTF16_LE`, `TestLoad_BOM_UTF16_BE`, the
  `Unmarshal` half of `TestMarshal`; scanner_test.go `TestScanner` with
  go1.27.0's expectations (C-WP115E-1); viper codec_test.go `TestCodec_Decode`
  (OK and InvalidData).
- Not ported: `TestStrictParse_PassThroughErrors` (needs a failing reader;
  viper passes a `bytes.Buffer`, which cannot fail); `TestLoad`,
  `TestMust_Load`, `TestLoad_overriding`, `TestApply`, `TestOverApply`,
  `TestOverLoad_overriding`, `TestOverLoad_overrideVars`,
  `TestOverLoad_overrideVars2` (they test setting process variables and the
  `override` path, D-WP115E-4; their parsing is covered by `TestRead` and the
  golden); `TestLoad_nonExist`, `TestMust_Load_nonExist`,
  `TestMustOverLoad_nonExist` (opening files, `Must`'s panic);
  `TestLoad_unicodeBOMFixture` (it scans the file with Go's default
  `ScanLines`, not gotenv); the `Marshal` half of `TestMarshal` and viper's
  `TestCodec_Encode` (encoding is not part of the decoder).
- Golden: `tests/dotenv_golden.rs` replays all 8793 cases through
  `strict_parse` (exact bytes) and `decode` (X-WP115E-1), and checks the named
  quirk cases. Unit tests cover Go's UTF-8 decoding against Rust's validator
  over every one- and two-byte input, `TrimSpace`, the x/text decoders, the
  probe's first read, the 4096-byte boundary and the 64 KiB limit.

## Mutation checks

24 single-point bugs, each applied alone with the file restored in a
`finally` block and a 900 s timeout per `cargo test -p gorse-config` run
(a Python script kept outside the repository; the hashes of the four source
files matched after the run). Every one made a test fail:

| Mutation | Caught by |
|---|---|
| M1 scanner's first buffer 8192 instead of 4096 | `cr_at_the_end_of_the_first_buffer`, golden, quirks |
| M2 BOM probe reads 4 bytes | `first_read_is_the_three_probe_bytes`, golden, quirks |
| M3 `MaxScanTokenSize` + 1 | golden, quirks |
| M4 nil final token counted (pre-1.22 `Scan`) | `gotenv_test_scanner`, `token_too_long` |
| M5 `splitLines` waits for the `\n` after a final `\r` | scanner unit tests, golden, quirks |
| M6 opening-quote check with the right index | golden, quirks |
| M7 `:` looked for before `=` | golden, quirks |
| M8 escaped quote on a continuation line missed | golden, `test_read` |
| M9 `\v` in Go's `\s` | `go_patterns_spelled_out`, golden |
| M10 `:` without required white space | `go_patterns_spelled_out`, golden, quirks |
| M11 `\n` escape not replaced first | golden, quirks, `test_parse`, `test_read` |
| M12 earlier key before the environment | golden, quirks, `test_load_override_vars` |
| M13 lower-case variable names | golden |
| M14 `parseExport` splits on tab | golden, quirks, `test_strict_parse` |
| M15 `checkFormat` text | golden, quirks, `test_load_env`, `test_codec_decode`, `test_strict_parse` |
| M16 U+0085 not a space | `trim_space_unicode_and_invalid`, golden |
| M17 `isHighSurrogate` on the lead range | `utf16_decoder_surrogates`, golden, quirks |
| M18 x/text UTF-8 decoder per byte | golden |
| M19 `transform.Reader` buffers of 2048 | golden |
| M20 single-quoted values expanded | golden, `test_read` |
| M21 Rust's lossy conversion for X-WP115E-1 | `go_string_replaces_each_invalid_byte`, `decode_matches_go_except_listed_invalid_utf8` |
| M22 `TrimSpace` keeps trailing space | `trim_space_unicode_and_invalid`, golden, `test_parse` |
| M23 "missing quotes" text | golden, quirks |
| M24 UTF-16 odd last byte not U+FFFD | `utf16_decoder_surrogates`, golden |

"golden" is `strict_parse_gives_go_bytes_for_every_case` and
`decode_matches_go_except_listed_invalid_utf8`; "quirks" is
`quirks_recorded_by_go`.
