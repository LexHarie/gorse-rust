// Copyright 2026 gorse Project Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Command yamlgolden records what spf13/viper v1.21.0's YAML codec
// (internal/encoding/yaml: yaml.Unmarshal of go.yaml.in/yaml/v3 v3.0.4 into
// a non-nil map[string]any) does with a corpus of inputs, for the Rust port
// in crates/gorse-config (formats::yaml):
//
//	go run ./contracts/tools/yamlgolden -out crates/gorse-config/tests/data/yaml_golden.json
//
// Each case records either the map the codec leaves behind or the error
// text. Values keep their exact Go types ({"t": "int", "v": "5"}; float64
// as the hex of its bits; time.Time as Time.String(); maps with their
// entries sorted). The corpus:
//
//   - the YAML inputs of yaml.v3's own tests, read from its source
//     (decode_test.go, limit_test.go, node_test.go, encode_test.go);
//   - the YAML files of crates/gorse-config/tests/data/config_golden.json;
//   - Gorse's config/config.toml written as YAML, twice: by a line-based
//     conversion that keeps the comments, and by yaml.Marshal;
//   - hand-written cases for every branch of resolve.go, tags, anchors,
//     aliases, merges, keys, multi-document streams, encodings and errors;
//   - seeded mutations (byte insertions, deletions and replacements) of the
//     inputs above that decode without error.
//
// time.Local is UTC for every case except those marked "local", which run
// with time.Local = time.FixedZone("CET", 3600) (time.Parse uses the local
// zone when an offset matches it).
//
// The file also holds the tables of yaml.v3's unit tests that decode into
// map[string]interface{} or report errors, with the expectations written in
// the Go test source (go_tests), for the Rust ports of those tests.
//
// The output is deterministic: a second run writes the same bytes.
package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"go.yaml.in/yaml/v3"
)

// Value is a Go value of static type any with its exact Go type.
type Value struct {
	// T is the Go type as reflect prints it, or "nil".
	T string `json:"t"`
	// V is the value: text for scalars, a list for slices and maps.
	V any `json:"v,omitempty"`
	// Hex holds a string's bytes when they are not UTF-8 (V then holds the
	// text with U+FFFD for the invalid bytes).
	Hex string `json:"hex,omitempty"`
}

// StringEntry is an entry of a map[string]interface{}.
type StringEntry struct {
	K    string `json:"k"`
	KHex string `json:"k_hex,omitempty"`
	V    *Value `json:"v"`
}

// AnyEntry is an entry of a map[interface{}]interface{}.
type AnyEntry struct {
	K *Value `json:"k"`
	V *Value `json:"v"`
}

// encode turns a value the decoder produced into a Value.
func encode(v any) *Value {
	switch v := v.(type) {
	case nil:
		return &Value{T: "nil"}
	case bool:
		return &Value{T: "bool", V: strconv.FormatBool(v)}
	case int:
		return &Value{T: "int", V: strconv.Itoa(v)}
	case int64:
		return &Value{T: "int64", V: strconv.FormatInt(v, 10)}
	case uint64:
		return &Value{T: "uint64", V: strconv.FormatUint(v, 10)}
	case float64:
		return &Value{T: "float64", V: fmt.Sprintf("%016x", math.Float64bits(v))}
	case string:
		out := &Value{T: "string", V: v}
		if !utf8.ValidString(v) {
			out.V = strings.ToValidUTF8(v, "�")
			out.Hex = hex.EncodeToString([]byte(v))
		}
		return out
	case time.Time:
		return &Value{T: "time.Time", V: v.String()}
	case []any:
		list := []*Value{}
		for _, x := range v {
			list = append(list, encode(x))
		}
		return &Value{T: "[]interface {}", V: list}
	case map[string]any:
		return &Value{T: "map[string]interface {}", V: stringEntries(v)}
	case map[any]any:
		type keyed struct {
			key, value string
			entry      AnyEntry
		}
		var list []keyed
		for k, x := range v {
			e := AnyEntry{K: encode(k), V: encode(x)}
			list = append(list, keyed{mustJSON(e.K), mustJSON(e.V), e})
		}
		sort.Slice(list, func(i, j int) bool {
			if list[i].key != list[j].key {
				return list[i].key < list[j].key
			}
			return list[i].value < list[j].value
		})
		entries := []AnyEntry{}
		for _, e := range list {
			entries = append(entries, e.entry)
		}
		return &Value{T: "map[interface {}]interface {}", V: entries}
	}
	panic(fmt.Sprintf("unexpected type %T", v))
}

func stringEntries(m map[string]any) []StringEntry {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	entries := []StringEntry{}
	for _, k := range keys {
		e := StringEntry{K: k, V: encode(m[k])}
		if !utf8.ValidString(k) {
			e.K = strings.ToValidUTF8(k, "�")
			e.KHex = hex.EncodeToString([]byte(k))
		}
		entries = append(entries, e)
	}
	return entries
}

func mustJSON(v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	return string(b)
}

// Part is a piece of a generated input: S repeated N times.
type Part struct {
	S string `json:"s"`
	N int    `json:"n"`
}

// Case is one input and what the codec does with it.
type Case struct {
	Name string `json:"name"`
	// Input is the input; InputHex holds it instead when it is not UTF-8,
	// and Gen when it is large (the concatenation of the parts).
	Input    *string `json:"input,omitempty"`
	InputHex string  `json:"input_hex,omitempty"`
	Gen      []Part  `json:"gen,omitempty"`
	// Local names the time.Local of the case ("CET" for FixedZone("CET",
	// 3600)); empty is UTC.
	Local string `json:"local,omitempty"`
	// Result is the decoded map; Error the error text; Panic a panic.
	// ResultText holds the result instead, as compact JSON, when it nests
	// deeper than maxDepth (JSON readers limit nesting); ResultSHA256 holds
	// the SHA-256 of that text, and ResultLen its length, when the text is
	// longer than maxText.
	Result       *Value `json:"result,omitempty"`
	ResultText   string `json:"result_text,omitempty"`
	ResultSHA256 string `json:"result_sha256,omitempty"`
	ResultLen    int    `json:"result_len,omitempty"`
	Error        string `json:"error,omitempty"`
	Panic        string `json:"panic,omitempty"`
}

// codecDecode is viper's (yaml.Codec).Decode.
func codecDecode(b []byte, v map[string]any) error {
	return yaml.Unmarshal(b, &v)
}

// decode runs the codec as viper.ReadInConfig does, with a fresh map.
func decode(b []byte) (result map[string]any, err error, panicked string) {
	defer func() {
		if r := recover(); r != nil {
			panicked = fmt.Sprint(r)
		}
	}()
	m := make(map[string]any)
	err = codecDecode(b, m)
	return m, err, ""
}

// run decodes the input of c and records the outcome.
func run(c *Case, input []byte) {
	if c.Local == "CET" {
		time.Local = time.FixedZone("CET", 3600)
	} else {
		time.Local = time.UTC
	}
	m, err, p := decode(input)
	time.Local = time.UTC
	switch {
	case p != "":
		c.Panic = p
	case err != nil:
		c.Error = err.Error()
	default:
		c.Result = encode(m)
		var b strings.Builder
		writeCompact(&b, c.Result)
		if b.Len() > maxText {
			sum := sha256.Sum256([]byte(b.String()))
			c.ResultSHA256 = hex.EncodeToString(sum[:])
			c.ResultLen = b.Len()
			c.Result = nil
		} else if depth(c.Result) > maxDepth {
			c.ResultText = b.String()
			c.Result = nil
		}
	}
}

// maxDepth is the deepest Value kept as JSON structure, and maxText the
// longest result kept in full.
const (
	maxDepth = 24
	maxText  = 16384
)

// depth is how deep values nest in v.
func depth(v *Value) int {
	d := 0
	switch x := v.V.(type) {
	case []*Value:
		for _, e := range x {
			d = max(d, depth(e))
		}
	case []StringEntry:
		for _, e := range x {
			d = max(d, depth(e.V))
		}
	case []AnyEntry:
		for _, e := range x {
			d = max(d, depth(e.K), depth(e.V))
		}
	}
	return d + 1
}

// writeCompact writes v as compact JSON with the field order of the
// structs: "t", "v", "hex"; "k", "k_hex", "v". Strings escape '"', '\\'
// and the bytes below 0x20 (as \u00XX) only.
func writeCompact(b *strings.Builder, v *Value) {
	b.WriteString(`{"t":`)
	writeString(b, v.T)
	switch x := v.V.(type) {
	case string:
		b.WriteString(`,"v":`)
		writeString(b, x)
	case []*Value:
		b.WriteString(`,"v":[`)
		for i, e := range x {
			if i > 0 {
				b.WriteString(",")
			}
			writeCompact(b, e)
		}
		b.WriteString("]")
	case []StringEntry:
		b.WriteString(`,"v":[`)
		for i, e := range x {
			if i > 0 {
				b.WriteString(",")
			}
			b.WriteString(`{"k":`)
			writeString(b, e.K)
			if e.KHex != "" {
				b.WriteString(`,"k_hex":`)
				writeString(b, e.KHex)
			}
			b.WriteString(`,"v":`)
			writeCompact(b, e.V)
			b.WriteString("}")
		}
		b.WriteString("]")
	case []AnyEntry:
		b.WriteString(`,"v":[`)
		for i, e := range x {
			if i > 0 {
				b.WriteString(",")
			}
			b.WriteString(`{"k":`)
			writeCompact(b, e.K)
			b.WriteString(`,"v":`)
			writeCompact(b, e.V)
			b.WriteString("}")
		}
		b.WriteString("]")
	}
	if v.Hex != "" {
		b.WriteString(`,"hex":`)
		writeString(b, v.Hex)
	}
	b.WriteString("}")
}

func writeString(b *strings.Builder, s string) {
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		switch c := s[i]; {
		case c == '"' || c == '\\':
			b.WriteByte('\\')
			b.WriteByte(c)
		case c < 0x20:
			fmt.Fprintf(b, "\\u%04x", c)
		default:
			b.WriteByte(c)
		}
	}
	b.WriteByte('"')
}

// corpus collects inputs, dropping repeats.
type corpus struct {
	cases []*Case
	seen  map[string]bool
	// inputs are the inputs of cases (nil for generated ones).
	inputs [][]byte
}

func newCorpus() *corpus {
	return &corpus{seen: map[string]bool{}}
}

// add records an input under a name unless it was seen before.
func (c *corpus) add(name string, input []byte) *Case {
	if c.seen[string(input)] {
		return nil
	}
	c.seen[string(input)] = true
	k := &Case{Name: name}
	if utf8.Valid(input) {
		s := string(input)
		k.Input = &s
	} else {
		k.InputHex = hex.EncodeToString(input)
	}
	run(k, input)
	c.cases = append(c.cases, k)
	c.inputs = append(c.inputs, input)
	return k
}

// addLocal records an input decoded with time.Local = CET.
func (c *corpus) addLocal(name string, input string) {
	s := input
	k := &Case{Name: name, Input: &s, Local: "CET"}
	run(k, []byte(input))
	c.cases = append(c.cases, k)
	c.inputs = append(c.inputs, nil)
}

// addGen records a generated input by its parts.
func (c *corpus) addGen(name string, parts []Part) {
	var b bytes.Buffer
	for _, p := range parts {
		b.WriteString(strings.Repeat(p.S, p.N))
	}
	input := b.Bytes()
	if c.seen[string(input)] {
		return
	}
	c.seen[string(input)] = true
	k := &Case{Name: name, Gen: parts}
	run(k, input)
	c.cases = append(c.cases, k)
	c.inputs = append(c.inputs, nil)
}

// GoUnmarshal is a row of TestUnmarshal whose target is
// map[string]interface{}: the input and the value the test expects.
type GoUnmarshal struct {
	Index int    `json:"index"`
	Input string `json:"input"`
	Want  *Value `json:"want"`
}

// GoError is a row of TestUnmarshalErrors: the input and the regular
// expression the error must match (gocheck's ErrorMatches anchors it).
// Applies says whether the codec's error matches it too; the rows that do
// not decode the document root into interface{} differently.
type GoError struct {
	Index    int    `json:"index"`
	Input    string `json:"input,omitempty"`
	InputHex string `json:"input_hex,omitempty"`
	Pattern  string `json:"pattern"`
	Applies  bool   `json:"applies"`
}

// GoTests are the yaml.v3 unit test tables the Rust port runs.
type GoTests struct {
	Unmarshal       []GoUnmarshal `json:"unmarshal"`
	UnmarshalErrors []GoError     `json:"unmarshal_errors"`
	// MergeTests and MergeTestsNested are the inputs of TestMerge and
	// TestMergeNestedStruct.
	MergeTests       string `json:"merge_tests"`
	MergeTestsNested string `json:"merge_tests_nested"`
	// Limits are the rows of TestLimits.
	Limits []GoLimit `json:"limits"`
	// FuzzCrashers are the inputs of TestFuzzCrashers.
	FuzzCrashers []string `json:"fuzz_crashers"`
	// Issue117 is the input of TestIssue117.
	Issue117 string `json:"issue117"`
}

// GoLimit is a row of TestLimits.
type GoLimit struct {
	Name  string `json:"name"`
	Gen   []Part `json:"gen"`
	Error string `json:"error,omitempty"`
}

// Golden is the output file.
type Golden struct {
	Module  string   `json:"module"`
	Cases   []*Case  `json:"cases"`
	GoTests *GoTests `json:"go_tests"`
}

func main() {
	out := flag.String("out", "", "output file")
	configGolden := flag.String("config-golden", "crates/gorse-config/tests/data/config_golden.json", "the config golden file whose YAML inputs join the corpus")
	configToml := flag.String("config-toml", "config/config.toml", "Gorse's config.toml")
	flag.Parse()
	if *out == "" {
		fmt.Fprintln(os.Stderr, "usage: yamlgolden -out <file>")
		os.Exit(2)
	}
	time.Local = time.UTC

	version, dir := moduleDir()
	tables := readTables(dir)
	c := newCorpus()
	for _, in := range tables.inputs {
		c.add(in.name, []byte(in.text))
	}
	for _, g := range tables.limits {
		c.addGen("limit_test.go:"+g.Name, g.Gen)
	}
	for _, in := range configInputs(*configGolden) {
		c.add(in.name, []byte(in.text))
	}
	for _, in := range configTomlAsYAML(*configToml) {
		c.add(in.name, []byte(in.text))
	}
	for _, in := range handWritten() {
		c.add(in.name, []byte(in.text))
	}
	for _, in := range localCases() {
		c.addLocal(in.name, in.text)
	}
	mutations(c)

	g := Golden{
		Module:  "go.yaml.in/yaml/v3 " + version,
		Cases:   c.cases,
		GoTests: goTests(tables),
	}
	if err := os.WriteFile(*out, render(g), 0o644); err != nil {
		panic(err)
	}
	counts := map[string]int{}
	for _, k := range c.cases {
		switch {
		case k.Panic != "":
			counts["panic"]++
		case k.Error != "":
			counts["error"]++
		default:
			counts["ok"]++
		}
	}
	fmt.Fprintf(os.Stderr, "%d cases: %d decoded, %d errors, %d panics\n", len(c.cases), counts["ok"], counts["error"], counts["panic"])
}

// render writes the golden file with one case per line.
func render(g Golden) []byte {
	line := func(v any) []byte {
		var buf bytes.Buffer
		enc := json.NewEncoder(&buf)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(v); err != nil {
			panic(err)
		}
		return bytes.TrimSuffix(buf.Bytes(), []byte("\n"))
	}
	var out bytes.Buffer
	out.WriteString("{\n\"module\": ")
	out.Write(line(g.Module))
	out.WriteString(",\n\"cases\": [\n")
	for i, c := range g.Cases {
		if i > 0 {
			out.WriteString(",\n")
		}
		out.Write(line(c))
	}
	out.WriteString("\n],\n\"go_tests\": ")
	out.Write(line(g.GoTests))
	out.WriteString("\n}\n")
	return out.Bytes()
}

// goTests builds the unit test tables, checking each error row against the
// codec.
func goTests(t *tables) *GoTests {
	g := &GoTests{
		Unmarshal:        t.unmarshal,
		MergeTests:       t.mergeTests,
		MergeTestsNested: t.mergeTestsNested,
		Limits:           t.limits,
		FuzzCrashers:     t.fuzzCrashers,
		Issue117:         t.issue117,
	}
	for _, row := range t.unmarshalErrors {
		_, err, _ := decode([]byte(row.input))
		re := regexp.MustCompile("^(?:" + row.pattern + ")$")
		e := GoError{Index: row.index, Pattern: row.pattern, Applies: err != nil && re.MatchString(err.Error())}
		if utf8.ValidString(row.input) {
			e.Input = row.input
		} else {
			e.InputHex = hex.EncodeToString([]byte(row.input))
		}
		g.UnmarshalErrors = append(g.UnmarshalErrors, e)
	}
	return g
}
