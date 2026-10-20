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

// Command tomlgolden writes what viper's TOML codec does with a corpus of
// documents, for the Rust port of go-toml (crates/gorse-config/src/formats/toml):
//
//	go run ./contracts/tools/tomlgolden -out crates/gorse-config/tests/data/toml_golden.json
//
// Run it from the repository root. Each case is a document and either the map
// toml.Unmarshal(b, &v) leaves in v (a map[string]any, as viper's codec
// decodes), every value with its exact Go type, or the error's text, with the
// position and context of a *toml.DecodeError.
//
// The corpus, in this order, without duplicates:
//
//   - the string literals of go-toml v2.2.4's own tests (every *_test.go of
//     the module, the toml-test cases of toml_testgen_test.go among them) and
//     its fuzz corpus, benchmark/benchmark.toml and benchmark/testdata/example.toml.gz;
//   - the toml-test suite as BurntSushi/toml v1.6.0 ships it
//     (internal/toml-test/tests, valid and invalid, TOML 1.0 and 1.1), and the
//     TOML blocks of the TOML 1.0.0 specification (internal/toml-test/specs/v1.0.0.md);
//     that module is read from the module cache as data
//     (go mod download github.com/BurntSushi/toml@v1.6.0);
//   - Gorse's config/config.toml and the config.toml texts of
//     crates/gorse-config/tests/data/config_golden.json;
//   - edge cases written for go-toml's code paths (cases.go);
//   - single-byte mutations (insert, delete, replace) of the documents above
//     that decode, from a fixed seed.
//
// The output is the same on every run.
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io"
	"io/fs"
	"math"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/pelletier/go-toml/v2"
)

// Node is one value of the decoded tree with its Go type (T is
// reflect.Type.String()). The tree is written flat, in pre-order, so that
// nesting does not nest the JSON: a map or a slice is followed by its N
// elements, the entries of a map sorted by key, each with its key K.
//
//   - int64: V is the decimal text;
//   - float64: V is the 16 hex digits of math.Float64bits;
//   - bool: V is the bool;
//   - string: V is the text, or Hex holds the bytes when they are not UTF-8;
//   - time.Time: V is t.String(), Time holds its zone and location;
//   - toml.LocalDate, toml.LocalTime, toml.LocalDateTime: V holds the fields;
//   - []interface {}, map[string]interface {}: N is the number of elements.
type Node struct {
	K    *string   `json:"k,omitempty"`
	KHex string    `json:"k_hex,omitempty"`
	T    string    `json:"t"`
	V    any       `json:"v,omitempty"`
	N    *int      `json:"n,omitempty"`
	Hex  string    `json:"hex,omitempty"`
	Time *TimeInfo `json:"time,omitempty"`
}

// TimeInfo is what Time.String does not show: the zone's name and offset
// (Time.Zone) and the location's name.
type TimeInfo struct {
	Zone     string `json:"zone"`
	Offset   int    `json:"offset"`
	Location string `json:"location"`
}

// Case is one document and what go-toml made of it.
type Case struct {
	Name string `json:"name"`
	// Input is the document, or InputHex when it is not UTF-8.
	Input    string `json:"input"`
	InputHex string `json:"input_hex,omitempty"`
	// Value is the decoded map; absent on an error.
	Value []Node `json:"value,omitempty"`
	// Error is err.Error().
	Error string `json:"error,omitempty"`
	// Decode is set when err is a *toml.DecodeError.
	Decode *DecodeError `json:"decode_error,omitempty"`
	// Panic is what Unmarshal panicked with.
	Panic string `json:"panic,omitempty"`
}

// DecodeError is a *toml.DecodeError's Position and String (Human, or
// HumanHex when it is not UTF-8).
type DecodeError struct {
	Line     int    `json:"line"`
	Column   int    `json:"column"`
	Human    string `json:"human"`
	HumanHex string `json:"human_hex,omitempty"`
}

// Golden is the output file.
type Golden struct {
	Cases []Case `json:"cases"`
}

// encode appends v and its elements to out, with key k when v is in a map.
func encode(out []Node, k *string, v any) []Node {
	t := reflect.TypeOf(v)
	if t == nil {
		panic("nil value")
	}
	n := Node{T: t.String()}
	if k != nil {
		if utf8.ValidString(*k) {
			n.K = k
		} else {
			empty := ""
			n.K, n.KHex = &empty, hex.EncodeToString([]byte(*k))
		}
	}
	switch x := v.(type) {
	case int64:
		n.V = strconv.FormatInt(x, 10)
	case float64:
		n.V = fmt.Sprintf("%016x", math.Float64bits(x))
	case bool:
		n.V = x
	case string:
		if utf8.ValidString(x) {
			n.V = x
		} else {
			n.V, n.Hex = "", hex.EncodeToString([]byte(x))
		}
	case time.Time:
		name, offset := x.Zone()
		n.V = x.String()
		n.Time = &TimeInfo{Zone: name, Offset: offset, Location: x.Location().String()}
	case toml.LocalDate:
		n.V = map[string]int{"year": x.Year, "month": x.Month, "day": x.Day}
	case toml.LocalTime:
		n.V = map[string]int{"hour": x.Hour, "minute": x.Minute, "second": x.Second,
			"nanosecond": x.Nanosecond, "precision": x.Precision}
	case toml.LocalDateTime:
		n.V = map[string]int{"year": x.Year, "month": x.Month, "day": x.Day,
			"hour": x.Hour, "minute": x.Minute, "second": x.Second,
			"nanosecond": x.Nanosecond, "precision": x.Precision}
	case []any:
		count := len(x)
		n.N = &count
		out = append(out, n)
		for _, e := range x {
			out = encode(out, nil, e)
		}
		return out
	case map[string]any:
		keys := make([]string, 0, len(x))
		for key := range x {
			keys = append(keys, key)
		}
		sort.Strings(keys)
		count := len(x)
		n.N = &count
		out = append(out, n)
		for _, key := range keys {
			out = encode(out, &key, x[key])
		}
		return out
	default:
		panic(fmt.Sprintf("unexpected type %T", v))
	}
	return append(out, n)
}

// decode runs viper's codec, toml.Unmarshal(b, &v) with v a map[string]any,
// on the bytes viper's unmarshalReader gives it: a bytes.Buffer filled with
// ReadFrom, which leaves spare capacity after the document. (go-toml finds
// positions by address, and Go points an empty slice at the very end of its
// array back to the slice's start, so with no spare capacity an error at the
// end of the document would be placed elsewhere.)
func decode(doc []byte) (c Case) {
	defer func() {
		if r := recover(); r != nil {
			c = Case{Panic: fmt.Sprint(r)}
		}
	}()
	buf := new(bytes.Buffer)
	if _, err := buf.ReadFrom(bytes.NewReader(doc)); err != nil {
		panic(err)
	}
	b := buf.Bytes()
	if cap(b) == len(b) {
		panic("no spare capacity")
	}
	v := map[string]any{}
	err := toml.Unmarshal(b, &v)
	if err != nil {
		c.Error = err.Error()
		if de, ok := err.(*toml.DecodeError); ok {
			line, column := de.Position()
			c.Decode = &DecodeError{Line: line, Column: column, Human: de.String()}
			if !utf8.ValidString(c.Decode.Human) {
				c.Decode.HumanHex, c.Decode.Human = hex.EncodeToString([]byte(c.Decode.Human)), ""
			}
		}
		return c
	}
	c.Value = encode(nil, nil, v)
	return c
}

// corpus collects documents in order, without duplicates.
type corpus struct {
	names []string
	docs  [][]byte
	seen  map[string]bool
}

func (c *corpus) add(name string, doc []byte) {
	if c.seen[string(doc)] {
		return
	}
	c.seen[string(doc)] = true
	c.names = append(c.names, name)
	c.docs = append(c.docs, append([]byte(nil), doc...))
}

// moduleDir is the module cache directory of a module.
func moduleDir(path, version string) string {
	out, err := exec.Command("go", "env", "GOMODCACHE").Output()
	if err != nil {
		panic(err)
	}
	escaped := ""
	for _, r := range path {
		if r >= 'A' && r <= 'Z' {
			escaped += "!" + string(r+'a'-'A')
		} else {
			escaped += string(r)
		}
	}
	dir := filepath.Join(strings.TrimSpace(string(out)), escaped+"@"+version)
	if _, err := os.Stat(dir); err != nil {
		panic(fmt.Sprintf("%v (run: go mod download %s@%s)", err, path, version))
	}
	return dir
}

// stringValue is the value of a constant string expression: a literal, or
// literals joined with +.
func stringValue(e ast.Expr) (string, bool) {
	switch x := e.(type) {
	case *ast.BasicLit:
		if x.Kind != token.STRING {
			return "", false
		}
		s, err := strconv.Unquote(x.Value)
		if err != nil {
			panic(err)
		}
		return s, true
	case *ast.ParenExpr:
		return stringValue(x.X)
	case *ast.BinaryExpr:
		if x.Op != token.ADD {
			return "", false
		}
		l, ok := stringValue(x.X)
		if !ok {
			return "", false
		}
		r, ok := stringValue(x.Y)
		if !ok {
			return "", false
		}
		return l + r, true
	}
	return "", false
}

// goStrings adds the string constants of a Go file.
func goStrings(c *corpus, root, path string) {
	fset := token.NewFileSet()
	f, err := parser.ParseFile(fset, path, nil, 0)
	if err != nil {
		panic(err)
	}
	rel, err := filepath.Rel(root, path)
	if err != nil {
		panic(err)
	}
	ast.Inspect(f, func(n ast.Node) bool {
		e, ok := n.(ast.Expr)
		if !ok {
			return true
		}
		if s, ok := stringValue(e); ok {
			c.add(fmt.Sprintf("go-toml %s:%d", rel, fset.Position(e.Pos()).Line), []byte(s))
		}
		return true
	})
}

// goToml adds go-toml's test strings, fuzz corpus and small benchmark files.
func goToml(c *corpus) {
	root := moduleDir("github.com/pelletier/go-toml/v2", "v2.2.4")
	var tests, fuzz []string
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if strings.HasSuffix(path, "_test.go") {
			tests = append(tests, path)
		}
		if strings.Contains(path, filepath.Join("testdata", "fuzz")) && !d.IsDir() {
			fuzz = append(fuzz, path)
		}
		return nil
	})
	if err != nil {
		panic(err)
	}
	for _, path := range tests {
		goStrings(c, root, path)
	}
	for _, path := range fuzz {
		text, err := os.ReadFile(path)
		if err != nil {
			panic(err)
		}
		lines := strings.Split(string(text), "\n")
		if lines[0] != "go test fuzz v1" {
			panic(path)
		}
		for _, line := range lines[1:] {
			if line == "" {
				continue
			}
			e, err := parser.ParseExpr(line)
			if err != nil {
				panic(err)
			}
			s, ok := stringValue(e.(*ast.CallExpr).Args[0])
			if !ok {
				panic(line)
			}
			c.add("go-toml fuzz "+filepath.Base(path), []byte(s))
		}
	}
	bench, err := os.ReadFile(filepath.Join(root, "benchmark", "benchmark.toml"))
	if err != nil {
		panic(err)
	}
	c.add("go-toml benchmark/benchmark.toml", bench)
	// The other datasets are megabytes of arrays of numbers.
	f, err := os.Open(filepath.Join(root, "benchmark", "testdata", "example.toml.gz"))
	if err != nil {
		panic(err)
	}
	defer func() { _ = f.Close() }()
	z, err := gzip.NewReader(f)
	if err != nil {
		panic(err)
	}
	example, err := io.ReadAll(z)
	if err != nil {
		panic(err)
	}
	c.add("go-toml benchmark/testdata/example.toml.gz", example)
}

// tomlTest adds the toml-test suite and the TOML 1.0.0 specification's
// examples from BurntSushi/toml.
func tomlTest(c *corpus) {
	root := filepath.Join(moduleDir("github.com/BurntSushi/toml", "v1.6.0"), "internal", "toml-test")
	err := filepath.WalkDir(filepath.Join(root, "tests"), func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() || !strings.HasSuffix(path, ".toml") {
			return nil
		}
		text, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		rel, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		c.add("toml-test "+filepath.ToSlash(rel), text)
		return nil
	})
	if err != nil {
		panic(err)
	}
	spec, err := os.ReadFile(filepath.Join(root, "specs", "v1.0.0.md"))
	if err != nil {
		panic(err)
	}
	lines := strings.Split(string(spec), "\n")
	for i := 0; i < len(lines); i++ {
		if lines[i] != "```toml" {
			continue
		}
		start := i + 1
		for i++; lines[i] != "```"; i++ {
		}
		block := strings.Join(lines[start:i], "\n") + "\n"
		c.add(fmt.Sprintf("toml 1.0.0 spec line %d", start), []byte(block))
	}
}

// gorse adds config/config.toml and the TOML texts of the config golden.
func gorse(c *corpus) {
	text, err := os.ReadFile("config/config.toml")
	if err != nil {
		panic(err)
	}
	c.add("gorse config/config.toml", text)
	data, err := os.ReadFile("crates/gorse-config/tests/data/config_golden.json")
	if err != nil {
		panic(err)
	}
	var golden struct {
		Load []struct {
			Name    string `json:"name"`
			File    string `json:"file"`
			Text    string `json:"text"`
			TextHex string `json:"text_hex"`
			Missing bool   `json:"missing"`
			Dir     bool   `json:"dir"`
		} `json:"load"`
	}
	if err := json.Unmarshal(data, &golden); err != nil {
		panic(err)
	}
	for _, l := range golden.Load {
		if l.File != "config.toml" || l.Missing || l.Dir {
			continue
		}
		text := []byte(l.Text)
		if l.TextHex != "" {
			if text, err = hex.DecodeString(l.TextHex); err != nil {
				panic(err)
			}
		}
		c.add("config golden "+l.Name, text)
	}
}

// alphabet are the bytes mutations insert and replace with, most of them
// meaningful to the parser.
var alphabet = []byte("[]{}=.,\"'\\#\n\r\t 0123456789+-_:eExobTtZz.infasluU\x00\x7f\xef\xbb\xbf\xc3\xa9\xff")

// mutate adds single-byte insertions, deletions and replacements of the
// documents that decode.
func mutate(c *corpus, results []Case, perDoc int) {
	rng := rand.New(rand.NewPCG(20261019, 115))
	n := len(c.docs)
	for i := 0; i < n; i++ {
		doc := c.docs[i]
		if results[i].Error != "" || results[i].Panic != "" || len(doc) == 0 || len(doc) > 4096 {
			continue
		}
		for j := 0; j < perDoc; j++ {
			var b byte
			if rng.IntN(8) == 0 {
				b = byte(rng.IntN(256))
			} else {
				b = alphabet[rng.IntN(len(alphabet))]
			}
			var m []byte
			var what string
			switch rng.IntN(3) {
			case 0:
				p := rng.IntN(len(doc) + 1)
				m = append(append(append([]byte{}, doc[:p]...), b), doc[p:]...)
				what = fmt.Sprintf("insert %#02x at %d", b, p)
			case 1:
				p := rng.IntN(len(doc))
				m = append(append([]byte{}, doc[:p]...), doc[p+1:]...)
				what = fmt.Sprintf("delete at %d", p)
			default:
				p := rng.IntN(len(doc))
				m = append([]byte{}, doc...)
				m[p] = b
				what = fmt.Sprintf("replace with %#02x at %d", b, p)
			}
			c.add(fmt.Sprintf("mutation of #%d: %s", i, what), m)
		}
	}
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()

	c := &corpus{seen: map[string]bool{}}
	goToml(c)
	tomlTest(c)
	gorse(c)
	for _, e := range edgeCases() {
		c.add("edge "+e.name, []byte(e.doc))
	}

	results := make([]Case, len(c.docs))
	for i, doc := range c.docs {
		results[i] = decode(doc)
	}
	base := len(c.docs)
	mutate(c, results, 6)
	for _, doc := range c.docs[base:] {
		results = append(results, decode(doc))
	}

	golden := Golden{}
	for i, doc := range c.docs {
		r := results[i]
		r.Name = c.names[i]
		if utf8.Valid(doc) {
			r.Input = string(doc)
		} else {
			r.InputHex = hex.EncodeToString(doc)
		}
		golden.Cases = append(golden.Cases, r)
	}

	var buf bytes.Buffer
	buf.WriteString("{\"cases\": [\n")
	for i, r := range golden.Cases {
		var line bytes.Buffer
		enc := json.NewEncoder(&line)
		enc.SetEscapeHTML(false)
		if err := enc.Encode(r); err != nil {
			panic(err)
		}
		buf.Write(bytes.TrimSuffix(line.Bytes(), []byte("\n")))
		if i+1 < len(golden.Cases) {
			buf.WriteString(",")
		}
		buf.WriteString("\n")
	}
	buf.WriteString("]}\n")
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		panic(err)
	}
}
