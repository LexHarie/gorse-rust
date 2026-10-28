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

// Command dotenvgolden writes what viper's dotenv codec does with a corpus of
// inputs, for the Rust port in crates/gorse-config/src/formats/dotenv.rs:
//
//	go run ./contracts/tools/dotenvgolden \
//	  -config-golden crates/gorse-config/tests/data/config_golden.json \
//	  -gotenv-dir "$(go env GOMODCACHE)/github.com/subosito/gotenv@v1.6.0" \
//	  -out crates/gorse-config/tests/data/dotenv_golden.json
//
// viper v1.21.0 reads a .env or .dotenv file with
// internal/encoding/dotenv.Codec.Decode: it copies the bytes into a
// bytes.Buffer, calls gotenv.StrictParse on it and sets v[key] = value for
// every variable. The codec is internal to viper, so decode repeats those
// lines; each error is also checked against viper.ReadConfig, which must
// report "While parsing config: " and the same text.
//
// The process environment is cleared before the first case. Each case names
// one of the environments listed in the output, the variables it sets and
// the ones it unsets (os.Setenv, os.Unsetenv), so gotenv's $VAR expansion
// sees only those.
//
// The corpus: gotenv's own tests and fixtures (the fixtures are embedded
// below; -gotenv-dir checks them against the module's files), the dotenv
// files of config_golden.json, cases for every branch of strictParse,
// parseLine, checkFormat, parseExport and varReplacement, generated lines,
// byte order marks (UTF-8, UTF-16LE/BE, odd lengths, unpaired surrogates),
// line breaks around the scanner's buffer sizes (4096 doubling to 65536) and
// the x/text reader's chunks, invalid UTF-8, and a seeded corpus of byte
// mutations. The output is the same on every run.
//
// A byte string is written as a JSON string when it is valid UTF-8 and at
// most 512 bytes long; as {"text": ..., "hex": ...} when it is not valid
// UTF-8 (text is what encoding/json makes of it, hex is the bytes); and when
// longer than 512 bytes as {"len": n, "rle": [[hex, count], ...]}, the
// concatenation of each hex piece repeated count times. Results list the
// variables sorted by key.
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math/rand/v2"
	"os"
	"path/filepath"
	"runtime"
	"runtime/debug"
	"slices"
	"sort"
	"strings"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/spf13/viper"
	"github.com/subosito/gotenv"
)

// Bytes is a Go string in the output (see the package comment).
type Bytes []byte

// longBytes is the length above which a string is run-length encoded.
const longBytes = 512

// MarshalJSON writes b as text, text and hex, or runs.
func (b Bytes) MarshalJSON() ([]byte, error) {
	switch {
	case len(b) > longBytes:
		return marshal(struct {
			Len int     `json:"len"`
			RLE [][]any `json:"rle"`
		}{len(b), runs(b)})
	case utf8.Valid(b):
		return marshal(string(b))
	default:
		return marshal(struct {
			Text string `json:"text"`
			Hex  string `json:"hex"`
		}{string(b), hex.EncodeToString(b)})
	}
}

// runs splits b into repeated units (1 to 32 bytes, at least 4 times and 32
// bytes) and the literal bytes between them, scanning from the left and
// taking at each position the unit that covers the most bytes.
func runs(b []byte) [][]any {
	var out [][]any
	lit := 0
	for i := 0; i < len(b); {
		bestP, bestK := 0, 0
		for p := 1; p <= 32 && i+p <= len(b); p++ {
			k := 1
			for i+(k+1)*p <= len(b) && bytes.Equal(b[i+k*p:i+(k+1)*p], b[i:i+p]) {
				k++
			}
			if k >= 4 && p*k >= 32 && p*k > bestP*bestK {
				bestP, bestK = p, k
			}
		}
		if bestP == 0 {
			i++
			continue
		}
		if lit < i {
			out = append(out, []any{hex.EncodeToString(b[lit:i]), 1})
		}
		out = append(out, []any{hex.EncodeToString(b[i : i+bestP]), bestK})
		i += bestP * bestK
		lit = i
	}
	if lit < len(b) {
		out = append(out, []any{hex.EncodeToString(b[lit:]), 1})
	}
	return out
}

// marshal is json.Marshal without HTML escaping and without the newline
// json.Encoder adds.
func marshal(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(buf.Bytes(), []byte("\n")), nil
}

// Entry is one variable of a result.
type Entry struct {
	Key   Bytes `json:"key"`
	Value Bytes `json:"value"`
}

// Case is one input and what the codec does with it.
type Case struct {
	Name  string `json:"name"`
	Input Bytes  `json:"input"`
	// Env names the environment in environments.
	Env string `json:"env"`
	// Result is the map the codec leaves, sorted by key; Error the error's
	// text. Exactly one is set.
	Result *[]Entry `json:"result,omitempty"`
	Error  *Bytes   `json:"error,omitempty"`
}

// spec is a case before it runs.
type spec struct {
	name  string
	input []byte
	env   string
}

// corpus collects the cases and rejects duplicate names.
type corpus struct {
	specs []spec
	names map[string]bool
}

func (c *corpus) add(name string, input []byte, env string) {
	if c.names[name] {
		panic("duplicate case name " + name)
	}
	if _, ok := environments[env]; !ok {
		panic(name + ": no environment " + env)
	}
	c.names[name] = true
	c.specs = append(c.specs, spec{name, slices.Clone(input), env})
}

func (c *corpus) addS(name, input string, env string) {
	c.add(name, []byte(input), env)
}

// decode is viper's dotenv Codec.Decode, without the map copy.
func decode(b []byte) (gotenv.Env, error) {
	var buf bytes.Buffer
	if _, err := buf.Write(b); err != nil {
		return nil, err
	}
	return gotenv.StrictParse(&buf)
}

// run runs a case in its environment.
func run(s spec) Case {
	e := environments[s.env]
	for _, k := range sortedKeys(e.Set) {
		if err := os.Setenv(k, e.Set[k]); err != nil {
			panic(err)
		}
	}
	for _, k := range e.Unset {
		if err := os.Unsetenv(k); err != nil {
			panic(err)
		}
	}
	defer func() {
		for k := range e.Set {
			if err := os.Unsetenv(k); err != nil {
				panic(err)
			}
		}
	}()
	if len(os.Environ()) != len(e.Set) {
		panic(fmt.Sprintf("%s: unexpected environment %q", s.name, os.Environ()))
	}

	c := Case{Name: s.name, Input: s.input, Env: s.env}
	env, err := decode(s.input)

	// viper must report the same error.
	v := viper.New()
	v.SetConfigType("env")
	verr := v.ReadConfig(bytes.NewReader(s.input))
	switch {
	case err == nil && verr != nil:
		panic(fmt.Sprintf("%s: viper fails: %v", s.name, verr))
	case err != nil && (verr == nil || verr.Error() != "While parsing config: "+err.Error()):
		panic(fmt.Sprintf("%s: viper says %v, gotenv %v", s.name, verr, err))
	}

	if err != nil {
		e := Bytes(err.Error())
		c.Error = &e
		return c
	}
	entries := []Entry{}
	for _, k := range sortedKeys(env) {
		entries = append(entries, Entry{Bytes(k), Bytes(env[k])})
	}
	c.Result = &entries
	return c
}

func sortedKeys[M ~map[string]string](m M) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

// Environment is what a case sets and unsets in the process environment.
type Environment struct {
	Set   map[string]string `json:"set,omitempty"`
	Unset []string          `json:"unset,omitempty"`
}

// environments are the environments the cases name.
var environments = map[string]Environment{
	"none":       {},
	"foo":        {Set: map[string]string{"FOO": "test"}},
	"no-foo":     {Unset: []string{"FOO"}},
	"no-foo-set": {Unset: []string{"FOO", "SET"}},
	"no-a-c":     {Unset: []string{"A", "C"}},
	"a-from-env": {Set: map[string]string{"A": "fromEnv"}, Unset: []string{"C"}},
	"c-from-env": {Set: map[string]string{"C": "fromEnv"}, Unset: []string{"A"}},
	"home":       {Set: map[string]string{"HOME": "/home/gorse"}},
	"no-home":    {Unset: []string{"HOME"}},
	"mix": {
		Set: map[string]string{
			"FOO": "test", "SET": "from-env", "EMPTY": "", "A": "env-a",
			"HOME": "/home/gorse", "U": "é€\u3000", "DOLLAR": "$FOO", "QUOTE": `"q'`,
		},
		Unset: []string{"BAR", "UNSET", "B", "C"},
	},
}

// gotenvFormats are the inputs of gotenv_test.go's formats table and whether
// the test sets FOO=test for them.
var gotenvFormats = []struct {
	in     string
	preset bool
}{
	{`FOO=bar`, false},
	{`FOO =bar`, false},
	{`FOO= bar`, false},
	{`  FOO=bar`, false},
	{`FOO=bar  `, false},
	{`FOO="bar"`, false},
	{`FOO="bar"  `, false},
	{`FOO='bar'`, false},
	{`FOO='bar'  `, false},
	{`FOO="escaped\"bar"`, false},
	{`FOO=`, false},
	{"FOO=test\nBAR=$FOO", false},
	{"FOO=test\nBAR=${FOO}bar", false},
	{`BAR=$FOO`, true},
	{`BAR=$FOO`, false},
	{"FOO=test\nBAR=\"quote $FOO\"", false},
	{"BAR='quote $FOO'", false},
	{`FOO="foo\$BAR"`, false},
	{`FOO="foo\${BAR}"`, false},
	{"FOO=test\nBAR=\"foo\\${FOO} ${FOO}\"", false},
	{"OPTION_A: 1", false},
	{"export OPTION_A=2", false},
	{"OPTION_A=2\nexport OPTION_A", false},
	{`FOO="bar\nbaz"`, false},
	{`FOO.BAR=foobar`, false},
	{`foo=bar `, false},
	{"\n \t  \nfoo=bar\n \nfizz=buzz", false},
	{"foo=bar # this is foo", false},
	{`foo="bar#baz" # comment`, false},
	{"\n\n\n # HERE GOES FOO \nfoo=bar", false},
	{`foo="ba#r"`, false},
	{"foo='ba#r'", false},
	{`foo="ba#r"  `, false},
	{`foo='ba#r'  `, false},
	{"FOO=bar\rbaz=fbb", false},
	{"FOO=bar\r\nbaz=fbb", false},
	{`FOO="bar\rbaz"`, false},
	{`FOO="bar\\$ \\$\\$"`, false},
	{`FOO="bar $ "`, false},
	{`FOO= bar`, false},
	{`foo= "bar#baz" # comment`, false},
	{`foo="---\na==\n---"`, false},
}

// gotenvErrorFormats are the inputs of gotenv_test.go's errorFormats table.
var gotenvErrorFormats = []string{
	"OPTION_A=2\nexport OH_NO_NOT_SET",
	`lol$wut`,
}

// gotenvMarshalText is what TestMarshal unmarshals.
const gotenvMarshalText = `EMPTY=""
FOO="BAR"
ONE=1
QUOTED="some \"quoted\" text"`

// gotenvFiles are gotenv v1.6.0's .env, .env.invalid and fixtures/*,
// byte for byte (checked with -gotenv-dir).
var gotenvFiles = []struct{ name, text string }{
	{".env", "HELLO=world\n"},
	{".env.invalid", "lol$wut\n"},
	{"fixtures/exported.env", "export OPTION_A=2\nexport OPTION_B='\\n'\n# This is a comment\nexport OPTION_C='The MIT License (MIT)\n\nCopyright (c) 2013 Alif Rachmawadi\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in\nall copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN\nTHE SOFTWARE.'\n\n"},
	{"fixtures/plain.env", "OPTION_A=1\nOPTION_B=2\nOPTION_C= 3\nOPTION_D =4\nOPTION_E = 5\n"},
	{"fixtures/quoted.env", "OPTION_A='1'\nOPTION_B='2'\nOPTION_C=''\nOPTION_D='\\n'\nOPTION_E=\"1\"\nOPTION_F=\"2\"\nOPTION_G=\"\"\nOPTION_H=\"\\n\"\nOPTION_I=\"some multi-line text\nwith \\\"escaped quotes\\\" and ${OPTION_A} variable\"\nOPTION_J='some$pecial$1$2!*chars=qweq\"\"e$$\\$\"\"'\nOPTION_K=\"\n\"\nOPTION_L=\"some multi-line text\nwith \\\"escaped quotes\\\"\nempty lines\n\nand ${OPTION_A} variable\n\"\n"},
	{"fixtures/utf16be_bom.env", "\xfe\xff\x00B\x00O\x00M\x00=\x00U\x00T\x00F\x00-\x001\x006\x00 \x00B\x00E\x00\r\x00\n"},
	{"fixtures/utf16le_bom.env", "\xff\xfeB\x00O\x00M\x00=\x00U\x00T\x00F\x00-\x001\x006\x00 \x00L\x00E\x00\r\x00\n\x00"},
	{"fixtures/utf8_bom.env", "\ufeffBOM=UTF-8"},
	{"fixtures/vars.env", "A=\"fromFile\"\nB=\"$A\"\n\nD=\"$C\"\nC=\"fromFile\"\n"},
	{"fixtures/yaml.env", "OPTION_A: 1\nOPTION_B: '2'\nOPTION_C: ''\nOPTION_D: '\\n'\n"},
}

// addGotenv adds gotenv's own test inputs and fixtures.
func addGotenv(c *corpus) {
	for i, f := range gotenvFormats {
		name := fmt.Sprintf("gotenv/formats/%02d", i)
		if f.preset {
			c.addS(name, f.in, "foo")
		} else {
			c.addS(name, f.in, "no-foo")
		}
	}
	for i, in := range gotenvErrorFormats {
		c.addS(fmt.Sprintf("gotenv/errorFormats/%d", i), in, "none")
	}
	c.addS("gotenv/TestMarshal", gotenvMarshalText, "none")
	for _, f := range gotenvFiles {
		c.addS("gotenv/"+f.name, f.text, "no-a-c")
	}
	// TestLoad_overrideVars and TestLoad_overrideVars2 set A and C.
	vars := gotenvFile("fixtures/vars.env")
	c.addS("gotenv/fixtures/vars.env A=fromEnv", vars, "a-from-env")
	c.addS("gotenv/fixtures/vars.env C=fromEnv", vars, "c-from-env")
}

// gotenvFile is an embedded gotenv file.
func gotenvFile(name string) string {
	for _, f := range gotenvFiles {
		if f.name == name {
			return f.text
		}
	}
	panic("no gotenv file " + name)
}

// checkGotenvFiles compares the embedded fixtures with the module's files.
func checkGotenvFiles(dir string) {
	for _, f := range gotenvFiles {
		b, err := os.ReadFile(filepath.Join(dir, f.name))
		if err != nil {
			panic(err)
		}
		if string(b) != f.text {
			panic("embedded copy of " + f.name + " differs from " + dir)
		}
	}
}

// addConfigGolden adds the dotenv files of config_golden.json.
func addConfigGolden(c *corpus, path string) {
	b, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	var golden struct {
		Load []struct {
			Name    string `json:"name"`
			File    string `json:"file"`
			Text    string `json:"text"`
			TextHex string `json:"text_hex"`
		} `json:"load"`
	}
	if err := json.Unmarshal(b, &golden); err != nil {
		panic(err)
	}
	n := 0
	for _, l := range golden.Load {
		if !strings.HasSuffix(l.File, ".env") && !strings.HasSuffix(l.File, ".dotenv") {
			continue
		}
		text := []byte(l.Text)
		if l.TextHex != "" {
			if text, err = hex.DecodeString(l.TextHex); err != nil {
				panic(err)
			}
		}
		c.add("config_golden/"+l.Name, text, "home")
		c.add("config_golden/"+l.Name+" (HOME unset)", text, "no-home")
		n++
	}
	if n == 0 {
		panic("no dotenv cases in " + path)
	}
}

// branches are inputs for each branch of strictParse, parseLine, checkFormat,
// parseExport and varReplacement.
var branches = []struct{ name, in string }{
	// strictParse: empty and comment lines.
	{"empty", ""},
	{"newline", "\n"},
	{"spaces", "   \t \n\r\n"},
	{"comment", "# c"},
	{"indented comment", "  \t# c=1\nA=1"},
	{"comment with quote", "# \"open\nA=1"},
	{"hash key", "#A=1"},
	{"unicode space line", "\u00a0\u3000\n\u2028A=1\u0085"},
	// strictParse: the delimiter and the opening quote.
	{"delimiter at 0", `="x`},
	{"colon at 0", `:"x`},
	{"delimiter last", `A=`},
	{"delimiter last then quote line", "A=\n\"x"},
	{"equals before colon", `A: "x=y`},
	{"colon before equals", `A:"x=y`},
	{"colon space quote", "A: \"x\ny\""},
	{"colon quote no space", `A:"x"`},
	{"quote closed same line", `A="x"`},
	{"quote empty", `A=""`},
	{"quote alone", `A="`},
	{"single quote alone", `A='`},
	{"quote escaped at end", "A=\"x\\\"\ny\""},
	{"quote escaped at end eof", `A="x\"`},
	{"single quote escaped at end", "A='x\\'\ny'"},
	{"quote with space after opening", `A=" x\"`},
	{"quote with spaces after opening", "A=\"  x\\\"\ny\""},
	{"quote index quirk", `A="  \"`},
	{"quote index quirk 2", "A=\"\t\\\"\"\nB=1"},
	{"quote after spaces", "A=   \"x\ny\""},
	{"quote unicode space", "A=\u00a0\"x\ny\""},
	{"single in double", `A="it's"`},
	{"double in single", `A='say "hi"'`},
	{"quote mid value", `A=x"y`},
	{"quote at end only", `A=x"`},
	{"two quoted", `A="x" "y"`},
	{"two single quoted", `A='x' 'y'`},
	{"quoted then text", `A="x" y`},
	{"quoted then comment", `A="x" # y`},
	{"quoted then hash", `A="x"#y`},
	{"quoted hash inside", `A="x#y" #z`},
	// strictParse: multi-line values.
	{"multi-line", "A=\"x\ny\""},
	{"multi-line single", "A='x\ny'"},
	{"multi-line empty lines", "A=\"x\n\n\ny\""},
	{"multi-line crlf", "A=\"x\r\ny\r\n\""},
	{"multi-line cr", "A=\"x\ry\r\""},
	{"multi-line cr cr lf", "A=\"x\r\r\ny\""},
	{"multi-line escaped", "A=\"x\ny\\\"\nz\""},
	{"multi-line escaped then real", "A=\"x\ny\\\" z\"\nB=1"},
	{"multi-line quote first", "A=\"x\n\"\nB=1"},
	{"multi-line backslash quote first", "A=\"x\n\\\"\n\"\nB=1"},
	{"multi-line trailing text", "A=\"x\ny\" z\nB=1"},
	{"multi-line trailing comment", "A=\"x\ny\" # z\nB=1"},
	{"multi-line other quote", "A=\"x\ny'\nz\""},
	{"multi-line missing", "A=\"x\ny"},
	{"multi-line missing newline", "A=\"x\n"},
	{"multi-line missing escaped", "A=\"x\ny\\\""},
	{"multi-line single missing", "A='x\ny"},
	{"multi-line joined then error", "A=\"x\ny\"z\n"},
	{"multi-line with export", "export A=\"x\ny\""},
	{"multi-line colon", "A: \"x\ny\""},
	{"multi-line keeps spaces", "A=\"  x  \n  y  \n  \""},
	{"multi-line hash", "A=\"x\n# y\n\""},
	{"multi-line value equals", "A=\"---\na==\n---\""},
	{"multi-line dollar", "A=\"x\n$FOO\n${SET}\""},
	{"multi-line single dollar", "A='x\n$FOO'"},
	// parseLine: keys and separators.
	{"key dots", "a.b.c=1"},
	{"key digits", "1=1\n2.3=2"},
	{"key dash", "A-B=1"},
	{"key space", "A B=1"},
	{"key unicode", "é=1"},
	{"key dot only", ".=1"},
	{"key underscore", "_=1"},
	{"key export", "export=1"},
	{"key exportA", "exportA=1"},
	{"key export dot", "export.x=1"},
	{"equals equals", "A==1"},
	{"equals colon", "A=:1"},
	{"colon equals", "A:=1"},
	{"colon no space", "A:1"},
	{"colon two spaces", "A:  1"},
	{"colon tab", "A:\t1"},
	{"colon newline value", "A:\n1"},
	{"space colon", "A :1"},
	{"space colon space", "A : 1"},
	{"colon colon", "A:: 1"},
	{"colon then colon", "A: 1: 2"},
	{"equals spaces", "A \t= \t1"},
	{"equals vertical tab", "A\v=1"},
	{"value vertical tab", "A=\v1"},
	{"value form feed", "A=\f1\f"},
	{"value nbsp", "A=\u00a01\u00a0"},
	{"value ideographic space", "A=\u30001"},
	{"value line separator", "A=1\u2028"},
	{"value nel", "A=\u00851"},
	{"key nbsp", "A\u00a0=1"},
	{"nbsp key", "\u00a0A=1"},
	{"value nul", "A=\x00"},
	{"value nul inside", "A=1\x002\nB=2"},
	{"value bom", "A=\ufeff1"},
	{"bom key", "B=1\n\ufeffA=1"},
	{"value only spaces", "A=   "},
	{"value hash", "A=#"},
	{"value hash text", "A=#x"},
	{"value text hash", "A=x#y"},
	{"value text space hash", "A=x #y"},
	{"value hash in single", "A='x#y'#z"},
	{"value spaces inside", "A=x  y  z"},
	{"value equals", "A=x=y"},
	{"value colon", "A=x:y"},
	{"value quote inside", "A=x'y'z"},
	{"value trailing quote", "A='x'y"},
	{"value one quote char", `A="`},
	{"value quote space", `A=" `},
	{"value two quotes", `A="'`},
	{"value tab inside", "A=x\ty"},
	{"duplicate key", "A=1\nA=2"},
	{"duplicate key order", "A=1\nB=2\nA=3"},
	// parseLine: double-quoted escapes.
	{"escape n", `A="x\ny"`},
	{"escape r", `A="x\ry"`},
	{"escape t", `A="x\ty"`},
	{"escape backslash", `A="x\\y"`},
	{"escape backslash n", `A="x\\ny"`},
	{"escape backslash r", `A="x\\ry"`},
	{"escape dollar", `A="x\$FOO"`},
	{"escape backslash dollar", `A="x\\$FOO"`},
	{"escape three backslashes dollar", `A="x\\\$FOO"`},
	{"escape quote", `A="x\"y"`},
	{"escape single quote", `A="x\'y"`},
	{"escape trailing backslash", `A="x\"`},
	{"escape lone backslash", `A="\"`},
	{"escape unicode", `A="\é\€"`},
	{"escape newline literal", "A=\"x\\\ny\""},
	{"escape in single", `A='x\ny\$FOO\\'`},
	{"escape unquoted", `A=x\ny\$FOO\\`},
	{"escape unquoted backslash dollar", `A=x\\$FOO`},
	{"escape unquoted brace", `A=\${FOO}`},
	// varReplacement.
	{"var plain", "A=$FOO"},
	{"var braces", "A=${FOO}"},
	{"var open brace", "A=${FOO"},
	{"var close brace", "A=$FOO}"},
	{"var empty braces", "A=${}"},
	{"var dollar", "A=$"},
	{"var dollar dollar", "A=$$"},
	{"var dollar brace", "A=${"},
	{"var lower", "A=$foo"},
	{"var lower braces", "A=${foo}"},
	{"var digits", "A=$1$23"},
	{"var dotted", "A=${a.b}"},
	{"var followed", "A=${FOO}x$FOO.y"},
	{"var adjacent", "A=$FOO$SET${EMPTY}$UNSET|"},
	{"var underscore", "A=$FOO_"},
	{"var empty env", "A=x${EMPTY}y"},
	{"var unset", "A=x${UNSET}y"},
	{"var earlier key", "B=1\nA=$B"},
	{"var earlier key shadowed by env", "SET=file\nA=$SET"},
	{"var earlier key empty", "B=\nA=x${B}y"},
	{"var later key", "A=$C\nC=1"},
	{"var self", "A=1\nA=${A}2"},
	{"var self env", "UNSET=1\nUNSET=${UNSET}2"},
	{"var value with dollar", "A=$DOLLAR"},
	{"var value with quotes", "A=$QUOTE"},
	{"var unicode value", "A=\"[$U]\""},
	{"var dotted key", "a.b=1\nA=${a.b}"},
	{"var in single", "A='$FOO ${FOO}'"},
	{"var in double", "A=\"$FOO ${FOO}\""},
	{"var escaped in double", "A=\"\\$FOO \\${FOO}\""},
	{"var escaped unquoted", "A=\\$FOO"},
	{"var after escape", "A=\"\\\\$FOO\""},
	{"var quoted mismatched", "A=\"$FOO'"},
	{"var home", "A=$HOME/x"},
	{"var key from env", "FOO=file\nA=$FOO"},
	// checkFormat and parseExport.
	{"bad line", "master.port"},
	{"bad line spaces", "  master.port  "},
	{"bad line after good", "A=1\nB"},
	{"bad lol", "lol$wut"},
	{"bad only equals", "="},
	{"bad only colon", ":"},
	{"bad space key", "A B"},
	{"export unset", "export B"},
	{"export set", "B=1\nexport B"},
	{"export set by env only", "export SET"},
	{"export bare", "export"},
	{"export space", "export "},
	{"export two spaces", "B=1\nexport  B"},
	{"export tab", "B=1\nexport\tB"},
	{"export two names", "B=1\nexport B C"},
	{"export key with dot", "a.b=1\nexport a.b"},
	{"export no space", "exportB"},
	{"export colon", "export B:"},
	{"export vertical tab", "export\vB=1"},
	{"export quoted", "export B=\"x"},
	{"export prefix word", "exporter B"},
	{"export indented", "B=1\n  export B  "},
	{"export empty key", "=1\nexport "},
	{"bad multi-line", "A=\"x\ny\" z w"},
	{"bad multi-line trailing spaces", "A=\"x\ny\" z \t"},
	{"bad after multi-line", "A=\"x\ny\"\nbad"},
	{"bad crlf", "A=1\r\nbad\r\n"},
	{"bad unicode", "é"},
	{"bad unicode space", "A\u00a0B=1"},
	{"error stops parsing", "A=1\nbad\nB=2"},
	{"error after quotes missing", "bad\nA=\"x"},
}

// addBranches adds the branch cases, each with and without the mixed
// environment.
func addBranches(c *corpus) {
	for _, b := range branches {
		c.addS("branch/"+b.name, b.in, "mix")
		c.addS("branch/"+b.name+" (no env)", b.in, "no-foo-set")
	}
}

// grid parts.
var (
	gridPrefixes = []string{"", " ", "\t", "export ", "export\t", "export  ", " export ", "\u00a0", "\v"}
	gridKeys     = []string{"A", "a.b", "A_1", "1", "_", ".", "é", "A-B", "", "export", "A B", "FOO"}
	gridSeps     = []string{"=", " =", "= ", " = ", ":", ": ", " : ", ":\t", "\t=\t", "==", ":=", "=:", ":  "}
	gridValues   = []string{
		"", "v", "v w", " v ", "'v'", `"v"`, "'v", `"v`, "v'", `'v"`, `"v'`, "''", `""`, "'", `"`,
		`"a\"b"`, `'a\'b'`, `"a\nb"`, `'a\nb'`, `a\nb`, "$FOO", "${FOO}", `"$FOO"`, "'$FOO'", `\$FOO`,
		`"\$FOO"`, "#", "a#b", `"a#b"`, "a=b", "a:b", `"x" y`, `"x"#y`, `"\\"`, `" \"`, "${SET}${UNSET}",
		"é€", "\u00a0v\u00a0", `"a\\nb"`, `"\'"`,
	}
	gridSuffixes = []string{"", " ", "\t", " # c", "#c", " #", ` "`, " '", "\u00a0"}
	gridTerms    = []string{"", "\n", "\r\n", "\r", "\n\n"}
)

func pick(r *rand.Rand, s []string) string { return s[r.IntN(len(s))] }

func gridLine(r *rand.Rand) string {
	return pick(r, gridPrefixes) + pick(r, gridKeys) + pick(r, gridSeps) + pick(r, gridValues) + pick(r, gridSuffixes)
}

// addGrid adds lines built from the grid parts: every value with every
// separator, every prefix with every key, then random lines and pairs.
func addGrid(c *corpus) {
	for i, v := range gridValues {
		for j, s := range gridSeps {
			c.addS(fmt.Sprintf("grid/value %02d sep %02d", i, j), "A"+s+v+"\n", "mix")
		}
	}
	for i, p := range gridPrefixes {
		for j, k := range gridKeys {
			c.addS(fmt.Sprintf("grid/prefix %d key %02d", i, j), p+k+"=v", "mix")
		}
	}
	r := rand.New(rand.NewPCG(0x646f74656e76, 1))
	for i := 0; i < 1500; i++ {
		c.addS(fmt.Sprintf("grid/line %04d", i), gridLine(r)+pick(r, gridTerms), "mix")
	}
	for i := 0; i < 600; i++ {
		n := 2 + r.IntN(3)
		var b strings.Builder
		for j := 0; j < n; j++ {
			b.WriteString(gridLine(r))
			b.WriteString(pick(r, gridTerms[1:]))
		}
		c.addS(fmt.Sprintf("grid/lines %04d", i), b.String(), "mix")
	}
}

// utf16Encode encodes s as UTF-16 with a byte order mark.
func utf16Encode(s string, le bool) []byte {
	return utf16Units(le, append([]uint16{0xFEFF}, utf16.Encode([]rune(s))...)...)
}

// utf16Units writes code units in the given byte order.
func utf16Units(le bool, units ...uint16) []byte {
	out := make([]byte, 0, 2*len(units))
	for _, u := range units {
		if le {
			out = append(out, byte(u), byte(u>>8))
		} else {
			out = append(out, byte(u>>8), byte(u))
		}
	}
	return out
}

func cat(parts ...[]byte) []byte { return bytes.Join(parts, nil) }

// addBOM adds byte order mark cases.
func addBOM(c *corpus) {
	bom8 := []byte("\xEF\xBB\xBF")
	texts := []struct{ name, text string }{
		{"empty", ""},
		{"simple", "A=1"},
		{"lines", "A=1\nB=\"x\ny\"\r\nC='z'\rD=$A"},
		{"unicode", "A=é€😀\u3000\nB=\"\u00a0x\u00a0\""},
		{"bom char", "\ufeffA=1"},
		{"bom char value", "A=\ufeff"},
		{"bad line", "A"},
		{"missing quotes", "A=\"x"},
		{"export", "export A=1\nexport A"},
	}
	for _, t := range texts {
		c.add("bom/utf8 "+t.name, cat(bom8, []byte(t.text)), "mix")
		c.add("bom/utf16le "+t.name, utf16Encode(t.text, true), "mix")
		c.add("bom/utf16be "+t.name, utf16Encode(t.text, false), "mix")
	}
	raw := []struct {
		name string
		in   []byte
	}{
		{"utf8 bom only", bom8},
		{"utf8 partial bom", []byte("\xEF\xBBA=1")},
		{"utf8 partial bom 1", []byte("\xEFA=1")},
		{"utf8 bom twice", cat(bom8, bom8, []byte("A=1"))},
		{"utf8 bom then utf16 bom", cat(bom8, []byte("\xFF\xFEA=1"))},
		{"utf8 bom invalid", []byte("\xEF\xBB\xBFA=\xFF\xC3(\xE2\x82\xF0\x9F\x98\nB=\xED\xA0\x80\xC0\xAF")},
		{"utf8 bom invalid at end", []byte("\xEF\xBB\xBFA=x\xE2\x82")},
		{"utf8 bom invalid line", []byte("\xEF\xBB\xBF\xFF=1")},
		{"utf8 bom latin1", []byte("\xEF\xBB\xBFA=caf\xE9")},
		{"utf16le bom only", []byte("\xFF\xFE")},
		{"utf16be bom only", []byte("\xFE\xFF")},
		{"utf16le bom one byte", []byte("\xFF\xFEA")},
		{"utf16be bom one byte", []byte("\xFE\xFFA")},
		{"utf16le odd length", append(utf16Encode("A=1", true), 'x')},
		{"utf16be odd length", append(utf16Encode("A=1", false), 'x')},
		{"utf16le odd length newline", append(utf16Encode("A=1\n", true), '\n')},
		{"utf16le high surrogate alone", utf16Units(true, 0xFEFF, 'A', '=', 0xD800, 'x')},
		{"utf16be high surrogate alone", utf16Units(false, 0xFEFF, 'A', '=', 0xD800, 'x')},
		{"utf16le low surrogate alone", utf16Units(true, 0xFEFF, 'A', '=', 0xDC00, 'x')},
		{"utf16le low low", utf16Units(true, 0xFEFF, 'A', '=', 0xDC00, 0xDC01, 'x')},
		{"utf16le high high", utf16Units(true, 0xFEFF, 'A', '=', 0xD800, 0xD801, 'x')},
		{"utf16le high high low", utf16Units(true, 0xFEFF, 'A', '=', 0xD800, 0xD801, 0xDC00, 'x')},
		{"utf16le low high", utf16Units(true, 0xFEFF, 'A', '=', 0xDC00, 0xD800, 'x')},
		{"utf16le pair", utf16Units(true, 0xFEFF, 'A', '=', 0xD83D, 0xDE00)},
		{"utf16le high at end", utf16Units(true, 0xFEFF, 'A', '=', 0xD800)},
		{"utf16le high at end odd", append(utf16Units(true, 0xFEFF, 'A', '=', 0xD800), 'x')},
		{"utf16le high then odd byte", append(utf16Units(true, 0xFEFF, 'A', '=', 0xD800, 'x'), 0)},
		{"utf16le low at end", utf16Units(true, 0xFEFF, 'A', '=', 0xDC00)},
		{"utf16le surrogate key", utf16Units(true, 0xFEFF, 0xD800, '=', '1')},
		{"utf16le fffe", utf16Units(true, 0xFEFF, 'A', '=', 0xFFFE)},
		{"utf16le bom twice", utf16Units(true, 0xFEFF, 0xFEFF, 'A', '=', '1')},
		{"utf16le then be bom", utf16Units(true, 0xFEFF, 0xFFFE, 'A', '=', '1')},
		{"utf16be with le content", cat([]byte("\xFE\xFF"), utf16Units(true, 'A', '=', '1'))},
		{"utf16le nul", utf16Units(true, 0xFEFF, 'A', '=', 0, '1')},
		{"utf16le crlf", utf16Units(true, 0xFEFF, 'A', '=', '"', 'x', '\r', '\n', 'y', '"')},
		{"utf16le cr", utf16Units(true, 0xFEFF, 'A', '=', '"', 'x', '\r', 'y', '"')},
		{"utf16le nbsp", utf16Units(true, 0xFEFF, 0xA0, 'A', '=', 0x3000, '1', 0x2028)},
		{"not a bom fe", []byte("\xFEA=1")},
		{"not a bom ff", []byte("\xFFA=1")},
		{"ff ff", []byte("\xFF\xFFA=1")},
		{"fe fe", []byte("\xFE\xFEA=1")},
		{"fffe after newline", []byte("A=1\n\xFF\xFEB=2")},
	}
	for _, r := range raw {
		c.add("bom/"+r.name, r.in, "mix")
	}
}

// addInvalid adds inputs that are not valid UTF-8 without a byte order mark.
func addInvalid(c *corpus) {
	raw := []struct{ name, in string }{
		{"value ff", "A=\xFF"},
		{"value quoted ff", "A=\"\xFF\""},
		{"value single ff", "A='\xFF'"},
		{"value truncated", "A=x\xE2\x82"},
		{"value truncated quoted", "A=\"x\xE2\x82\""},
		{"value surrogate bytes", "A=\xED\xA0\x80"},
		{"value overlong", "A=\xC0\xAF"},
		{"value latin1", "A=caf\xE9"},
		{"value continuation", "A=\x80\x81"},
		{"value five bytes", "A=\xF8\x88\x80\x80\x80"},
		{"value f4 90", "A=\xF4\x90\x80\x80"},
		{"value escape ff", "A=\"\\\xFF\""},
		{"value escape truncated", "A=\"\\\xE2\x82\""},
		{"value ff space", "A=\xFF \t"},
		{"value ff nbsp", "A=\xFF\u00a0"},
		{"value nbsp truncated", "A=\xC2"},
		{"value multi-line", "A=\"\xFF\n\xFE\""},
		{"key ff", "\xFF=1"},
		{"key ff after", "A\xFF=1"},
		{"line ff", "\xFF"},
		{"line ff spaces", "  \xFF  "},
		{"export ff", "export \xFF"},
		{"export ff set", "export A\xFF"},
		{"quote ff", "A=\"\xFF"},
		{"comment ff", "# \xFF\nA=1"},
		{"env ff", "A=$FOO\xFF"},
		{"bad after ff", "A=\xFF\nbad"},
	}
	for _, r := range raw {
		c.addS("invalid/"+r.name, r.in, "mix")
	}
}

// addBoundaries adds line breaks around the scanner's buffer boundaries.
func addBoundaries(c *corpus) {
	prefixes := []string{"", "B=1\n", "BB=22\r\n", "# c\n\n", "B=\"m\nn\"\n"}
	bases := []int{4096, 8192, 16384, 32768, 65536}
	type variant struct {
		name       string
		head, tail string
		// lineStart is the offset of the line holding the break, from the
		// end of the prefix.
		lineStart int
	}
	variants := []variant{
		{"quoted crlf", `A="`, "\r\n" + `y"` + "\n", 0},
		{"continuation crlf", "A=\"\n", "\r\n" + `y"` + "\n", 4},
		{"quoted cr", `A="`, "\r" + `y"`, 0},
		{"quoted lf", `A="`, "\n" + `y"`, 0},
		{"unquoted crlf", "A=", "\r\nC=2", 0},
		{"quoted crlf eof", `A="`, "\r\n", 0},
	}
	for pi, p := range prefixes {
		for _, base := range bases {
			for delta := -3; delta <= 2; delta++ {
				for _, v := range variants {
					// The offset of the break from the line's start.
					at := base - 1 + delta
					pad := at - (len(v.head) - v.lineStart)
					in := p + v.head + strings.Repeat("x", pad) + v.tail
					c.addS(fmt.Sprintf("boundary/prefix %d %s %d%+d", pi, v.name, base, delta), in, "none")
				}
			}
		}
	}
	// Several lines whose breaks fall at many offsets.
	r := rand.New(rand.NewPCG(0x626f756e64, 2))
	for i := 0; i < 60; i++ {
		var b strings.Builder
		b.WriteString(`A="`)
		for b.Len() < 4096*3 {
			b.WriteString(strings.Repeat("x", r.IntN(200)))
			b.WriteString(pick(r, []string{"\r\n", "\r\n", "\n", "\r"}))
		}
		b.WriteString(`z"`)
		c.addS(fmt.Sprintf("boundary/random lines %02d", i), b.String(), "none")
	}
	// Through the x/text reader: UTF-8 with a BOM, UTF-16LE and UTF-16BE.
	// The UTF-16 reader hands the scanner 2048 bytes per read for ASCII.
	for _, enc := range []string{"utf8", "utf16le", "utf16be"} {
		for _, base := range []int{2048, 4096, 6144, 8192, 65536} {
			for delta := -3; delta <= 2; delta++ {
				for _, v := range variants[:3] {
					at := base - 1 + delta
					pad := at - (len(v.head) - v.lineStart)
					text := v.head + strings.Repeat("x", pad) + v.tail
					var in []byte
					switch enc {
					case "utf8":
						in = cat([]byte("\xEF\xBB\xBF"), []byte(text))
					case "utf16le":
						in = utf16Encode(text, true)
					default:
						in = utf16Encode(text, false)
					}
					c.add(fmt.Sprintf("boundary/%s %s %d%+d", enc, v.name, base, delta), in, "none")
				}
			}
		}
	}
	// Multi-byte characters through the x/text reader: three UTF-8 bytes for
	// two UTF-16 bytes fill its 4096-byte buffer before the source does, and
	// surrogate pairs and UTF-8 sequences straddle its source buffer.
	for pad := 0; pad < 8; pad++ {
		for _, ch := range []string{"€", "😀", "é"} {
			text := `A="` + strings.Repeat("x", pad) + strings.Repeat(ch, 6000/len(ch)) + "\r\n" + `y"`
			c.add(fmt.Sprintf("boundary/wide utf16le %s pad %d", ch, pad), utf16Encode(text, true), "none")
			c.add(fmt.Sprintf("boundary/wide utf8 %s pad %d", ch, pad), cat([]byte("\xEF\xBB\xBF"), []byte(text)), "none")
		}
		in := cat([]byte("\xEF\xBB\xBFA=\""), bytes.Repeat([]byte("x"), 4090+pad), []byte("\xE2\x82\xFF\xF0\x9F\x98"), bytes.Repeat([]byte("\xE2"), 1400), []byte("\r\ny\""))
		c.add(fmt.Sprintf("boundary/invalid utf8 pad %d", pad), in, "none")
	}
}

// addLong adds lines around bufio.MaxScanTokenSize.
func addLong(c *corpus) {
	for _, n := range []int{4094, 4095, 4096, 4097, 8191, 8192, 65533, 65534, 65535, 65536, 65537, 70000} {
		for _, term := range []struct{ name, s string }{{"eof", ""}, {"lf", "\n"}, {"crlf", "\r\n"}, {"cr", "\r"}} {
			line := "A=" + strings.Repeat("x", n-2)
			c.addS(fmt.Sprintf("long/line %d %s", n, term.name), line+term.s, "none")
			c.addS(fmt.Sprintf("long/line %d %s then B", n, term.name), line+term.s+"B=1", "none")
			c.addS(fmt.Sprintf("long/B then line %d %s", n, term.name), "B=1\n"+line+term.s, "none")
		}
		c.addS(fmt.Sprintf("long/continuation %d", n), "A=\"\n"+strings.Repeat("x", n)+"\n\"", "none")
		c.addS(fmt.Sprintf("long/comment %d", n), "#"+strings.Repeat("x", n-1)+"\nA=1", "none")
		c.addS(fmt.Sprintf("long/spaces %d", n), strings.Repeat(" ", n)+"\nA=1", "none")
		c.add(fmt.Sprintf("long/utf16le line %d", n), utf16Encode("A="+strings.Repeat("x", n-2)+"\n", true), "none")
		c.add(fmt.Sprintf("long/utf8 bom line %d", n), cat([]byte("\xEF\xBB\xBFA="), bytes.Repeat([]byte("x"), n-2), []byte("\n")), "none")
	}
	// A value longer than a token, made of short lines.
	c.addS("long/multi-line value", "A=\""+strings.Repeat("0123456789abcdef\n", 5000)+"\"", "none")
	c.addS("long/many keys", func() string {
		var b strings.Builder
		for i := 0; i < 3000; i++ {
			fmt.Fprintf(&b, "K%d=${K%d}%d\n", i, i/2, i%10)
		}
		return b.String()
	}(), "none")
}

// mutationBytes are what mutations insert or write: mostly the characters
// gotenv cares about, sometimes bytes that are not ASCII.
var (
	mutationASCII = []byte("=:\"'\\${}# \t\n\r\vAB_.xpo1")
	mutationOther = []byte{0x00, 0x0c, 0x80, 0xA0, 0xC2, 0xE2, 0xE3, 0xEF, 0xBB, 0xBF, 0xFE, 0xFF, 0x85, 0xED, 0xD8, 0xDC}
)

// addMutations adds seeded byte mutations of the small inputs so far.
func addMutations(c *corpus) {
	var seeds [][]byte
	for _, s := range c.specs {
		if len(s.input) > 0 && len(s.input) <= 300 {
			seeds = append(seeds, s.input)
		}
	}
	r := rand.New(rand.NewPCG(0x6d75746174, 3))
	mutByte := func() byte {
		if r.IntN(100) < 8 {
			return mutationOther[r.IntN(len(mutationOther))]
		}
		return mutationASCII[r.IntN(len(mutationASCII))]
	}
	for i := 0; i < 4000; i++ {
		in := slices.Clone(seeds[r.IntN(len(seeds))])
		for n := 1 + r.IntN(4); n > 0; n-- {
			switch op := r.IntN(3); {
			case op == 0 || len(in) == 0:
				at := r.IntN(len(in) + 1)
				in = slices.Insert(in, at, mutByte())
			case op == 1:
				at := r.IntN(len(in))
				in = slices.Delete(in, at, at+1)
			default:
				in[r.IntN(len(in))] = mutByte()
			}
		}
		c.add(fmt.Sprintf("mutation/%04d", i), in, "mix")
	}
}

func main() {
	out := flag.String("out", "", "output JSON file")
	configGolden := flag.String("config-golden", "", "config_golden.json, whose dotenv files join the corpus")
	gotenvDir := flag.String("gotenv-dir", "", "gotenv's module directory, to check the embedded fixtures")
	flag.Parse()
	if *out == "" || *configGolden == "" {
		flag.Usage()
		os.Exit(2)
	}
	if *gotenvDir != "" {
		checkGotenvFiles(*gotenvDir)
	}
	os.Clearenv()

	c := &corpus{names: map[string]bool{}}
	addGotenv(c)
	addConfigGolden(c, *configGolden)
	addBranches(c)
	addGrid(c)
	addBOM(c)
	addInvalid(c)
	addBoundaries(c)
	addLong(c)
	addMutations(c)

	var buf bytes.Buffer
	buf.WriteString("{\n")
	fmt.Fprintf(&buf, "  \"go\": %q,\n", runtime.Version())
	for _, mod := range []string{"github.com/subosito/gotenv", "golang.org/x/text", "github.com/spf13/viper"} {
		fmt.Fprintf(&buf, "  %q: %q,\n", mod, moduleVersion(mod))
	}
	envs, err := marshal(environments)
	if err != nil {
		panic(err)
	}
	fmt.Fprintf(&buf, "  \"environments\": %s,\n", envs)
	buf.WriteString("  \"cases\": [\n")
	for i, s := range c.specs {
		b, err := marshal(run(s))
		if err != nil {
			panic(err)
		}
		buf.WriteString("    ")
		buf.Write(b)
		if i+1 < len(c.specs) {
			buf.WriteString(",")
		}
		buf.WriteString("\n")
	}
	buf.WriteString("  ]\n}\n")
	if err := os.WriteFile(*out, buf.Bytes(), 0o644); err != nil {
		panic(err)
	}
}

// moduleVersion is the version of a module the tool is built with.
func moduleVersion(path string) string {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		panic("no build info")
	}
	for _, d := range info.Deps {
		if d.Path == path {
			return d.Version
		}
	}
	panic("no module " + path)
}
