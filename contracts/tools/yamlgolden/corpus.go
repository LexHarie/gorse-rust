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

package main

// The corpus beyond yaml.v3's own tests: the YAML files of the config
// golden, Gorse's config.toml written as YAML, hand-written cases and
// seeded mutations.

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/rand"
	"os"
	"strings"
	"unicode/utf16"

	"github.com/pelletier/go-toml/v2"
	"go.yaml.in/yaml/v3"
)

// configInputs reads the YAML files of the config golden.
func configInputs(path string) []namedInput {
	data, err := os.ReadFile(path)
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
	if err := json.Unmarshal(data, &golden); err != nil {
		panic(err)
	}
	var out []namedInput
	for _, c := range golden.Load {
		if c.File != "config.yaml" && c.File != "config.yml" {
			continue
		}
		text := c.Text
		if c.TextHex != "" {
			b, err := hex.DecodeString(c.TextHex)
			if err != nil {
				panic(err)
			}
			text = string(b)
		}
		out = append(out, namedInput{"config_golden:" + c.Name, text})
	}
	return out
}

// configTomlAsYAML writes Gorse's config.toml as YAML: line by line keeping
// the comments, and through yaml.Marshal of the decoded TOML.
func configTomlAsYAML(path string) []namedInput {
	data, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	var m map[string]any
	if err := toml.Unmarshal(data, &m); err != nil {
		panic(err)
	}
	marshaled, err := yaml.Marshal(m)
	if err != nil {
		panic(err)
	}
	return []namedInput{
		{"config.toml as YAML (comments kept)", tomlLinesToYAML(string(data))},
		{"config.toml as YAML (yaml.Marshal)", string(marshaled)},
	}
}

// tomlLinesToYAML converts the subset of TOML config.toml uses: comments,
// [table] and [[array.of.tables]] headers, key = value lines with strings,
// literal strings, multi-line basic strings, numbers, booleans and inline
// arrays.
func tomlLinesToYAML(text string) string {
	var b strings.Builder
	emitted := map[string]bool{}
	indent := "" // of the keys of the current table
	first := false
	itemIndent := ""
	lines := strings.Split(text, "\n")
	for i := 0; i < len(lines); i++ {
		line := strings.TrimSpace(lines[i])
		switch {
		case line == "":
			b.WriteString("\n")
		case strings.HasPrefix(line, "#"):
			b.WriteString(indent + line + "\n")
		case strings.HasPrefix(line, "[[") || strings.HasPrefix(line, "["):
			array := strings.HasPrefix(line, "[[")
			path := strings.Split(strings.Trim(line, "[]"), ".")
			for j := range path {
				key := strings.Join(path[:j+1], ".")
				last := j == len(path)-1
				if !emitted[key] || (last && !array) {
					b.WriteString(strings.Repeat("  ", j) + path[j] + ":\n")
					emitted[key] = true
				}
			}
			if array {
				itemIndent = strings.Repeat("  ", len(path)-1) + "  - "
				indent = strings.Repeat("  ", len(path)-1) + "    "
				first = true
			} else {
				indent = strings.Repeat("  ", len(path))
				first = false
			}
		default:
			key, value, ok := strings.Cut(line, " = ")
			if !ok {
				panic("unexpected line: " + line)
			}
			prefix := indent
			if first {
				prefix = itemIndent
				first = false
			}
			if value == `"""` {
				b.WriteString(prefix + key + ": |\n")
				for i++; strings.TrimSpace(lines[i]) != `"""`; i++ {
					if lines[i] == "" {
						b.WriteString("\n")
					} else {
						b.WriteString(indent + "  " + lines[i] + "\n")
					}
				}
				continue
			}
			b.WriteString(prefix + key + ": " + value + "\n")
		}
	}
	return b.String()
}

// resolveScalars are plain scalars for every branch of resolve.go.
var resolveScalars = []string{
	"", "~", "null", "Null", "NULL", "nULL", "nulL", "Nil", "none",
	"true", "True", "TRUE", "tRUE", "false", "False", "FALSE", "fALSE",
	"yes", "Yes", "YES", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF",
	"y", "Y", "n", "N", "t", "f", "o", "O",
	".nan", ".NaN", ".NAN", ".nAn", "nan", "NaN", "+.nan", "-.nan",
	".inf", ".Inf", ".INF", ".iNf", "+.inf", "+.Inf", "+.INF", "-.inf", "-.Inf", "-.INF",
	"inf", "Inf", "+inf", "-inf", "infinity", "-Infinity", ".infinity",
	"0", "-0", "+0", "00", "000", "01", "07", "08", "09", "010", "0129", "0777", "-0777", "+0777",
	"0o17", "0O17", "0o8", "-0o17", "+0o17", "0o-17", "0o+17", "-0o-17", "0o", "-0o",
	"0b101", "0B101", "0b2", "-0b101", "+0b101", "0b-101", "0b+101", "-0b-1", "0b", "-0b",
	"0x1F", "0X1F", "0x1f", "-0x1F", "+0x1F", "0x-1F", "0x", "0xG", "0x_1F", "0x1_F",
	"1_000", "1__000", "_1", "1_", "1_000_000", "-1_0", "+_1", "0_7", "0_8", "0b1_0", "0o1_7",
	"1", "+1", "-1", "12", "123456789",
	"9223372036854775807", "9223372036854775808", "-9223372036854775808", "-9223372036854775809",
	"18446744073709551615", "18446744073709551616", "-18446744073709551615",
	"0x7FFFFFFFFFFFFFFF", "0x8000000000000000", "0xFFFFFFFFFFFFFFFF", "0x10000000000000000",
	"-0x8000000000000000", "-0x8000000000000001",
	"0b" + strings.Repeat("1", 63), "0b" + strings.Repeat("1", 64), "0b" + strings.Repeat("1", 65),
	"-0b1" + strings.Repeat("0", 63), "-0b1" + strings.Repeat("0", 64),
	"0o777777777777777777777", "0o1777777777777777777777", "0o2000000000000000000000",
	"-0o1000000000000000000000", "-0o1000000000000000000001",
	"0777777777777777777777", "01777777777777777777777", "02000000000000000000000",
	"1.5", "-1.5", "+1.5", "1.", "-1.", ".5", "+.5", "-.5", "0.", "-0.0", "+0.0", "-.0", "0.0",
	"1e3", "1E3", "1e+3", "1e-3", "1.5e3", ".5e3", "1.e3", "-1e3", "+1e3", "1e", "e3", "1e3.5",
	"1.2.3", "1_000.5", ".5_5", "._5", "._", "1_.5", "1._5", "1e1_0", "1_e3", ".5e1_0",
	"1e400", "-1e400", "1e-400", "4.9e-324", "2.5e-324", "2.4e-324",
	"1.7976931348623157e308", "1.7976931348623159e308", "179769313486231580793728971405303415079934132710037826936173778980444968292764750946649017977587207096330286416692887910946555547851940402630657488671505820681908902000708383676273854845817711531764475730270069855571366959622842914819860834936475292719074168444365510704342711559699508093042880177904174497791",
	"0x1p-2", "0x1.8p1", "1,000", "1 000", "12:30", "1:20:30", "190:20:30.15",
	"123456e1", "6.8523e+5", "685.23015e+03", "685_230.15", "685230", "+685_230", "02472256", "0x_0A_74_AE", "0b1010_0111_0100_1010_1110",
	"2001-12-14", "2001-12-14t21:59:43.10-05:00", "2001-12-14T21:59:43.10Z", "2001-12-14T21:59:43.10-05:00",
	"2001-12-14 21:59:43.10 -5", "2001-12-14 21:59:43.10", "2001-12-14 21:59:43", "2001-12-14  21:59:43",
	"2001-12-14T21:59:43", "2001-12-14t21:59:43Z", "2001-1-2", "2001-1-2T3:4:5Z", "2001-13-01", "2001-02-30",
	"2001-02-29", "2000-02-29", "20011-01-01", "201-01-01", "2001-01-01T25:00:00Z", "2001-01-01T10:00:00+01:00",
	"2001-01-01T10:00:00-00:00", "2001-01-01T10:00:00+00:00", "2001-01-01T10:00:00+14:00", "2001-01-01T10:00:00+24:00",
	"2001-01-01 10:00:00.123456789", "2001-01-01 10:00:00.1234567891", "2001-01-01 10:00:00.",
	"2001-01-01T10:00:00.5+05:30", "2001-01-01T10:00:00z", "+2001-01-01", "-2001-01-01", "2001-01-01x",
	"0001-01-01", "9999-12-31", "0000-01-01", "2001-01", "2001-", "2001", "2001-01-01T", "2001-01-01 ",
	"2015-02-24T18:19:39.123456789-03:00", "2015-02-24T18:19:39Z", "1999-12-31T23:59:60Z",
	"<<", "=", "foo", "é", "1é", "--", "-a", "+", "+-1", "--1", ".", "..", ".1.", "...", "-.", "+.",
	"0.1.2", "1-2", "1+2", "1/2", "true1", "nulls", "~~", "~a", "a~",
}

// tags for scalars, in their source forms.
var scalarTags = []string{
	"!!str", "!!int", "!!float", "!!bool", "!!null", "!!timestamp", "!!binary",
	"!!seq", "!!map", "!!merge", "!!foo", "!foo", "!",
	"!<tag:yaml.org,2002:int>", "!<tag:yaml.org,2002:str>", "!<!foo>",
	"!<tag:example.com,2000:foo>", "!!%69nt", "!!in%74", "!e!int",
}

var taggedScalars = []string{
	"", "1", "-1", "1.5", "0x10", "1_000", "0b101", "0o17", "017", "true", "yes", "~", "null", "x",
	"2001-01-01", "2001-01-01T00:00:00Z", ".inf", ".nan", "-.inf", "aGVsbG8=", "<<",
	"9223372036854775808", "-9223372036854775809", "1e400", "0129",
}

func utf16le(s string, bom bool) string {
	var b []byte
	if bom {
		b = append(b, 0xff, 0xfe)
	}
	for _, u := range utf16.Encode([]rune(s)) {
		b = append(b, byte(u), byte(u>>8))
	}
	return string(b)
}

func utf16be(s string, bom bool) string {
	var b []byte
	if bom {
		b = append(b, 0xfe, 0xff)
	}
	for _, u := range utf16.Encode([]rune(s)) {
		b = append(b, byte(u>>8), byte(u))
	}
	return string(b)
}

// handWritten are the cases written for this port.
func handWritten() []namedInput {
	var out []namedInput
	add := func(name, text string) {
		out = append(out, namedInput{"hand:" + name, text})
	}
	for i, s := range resolveScalars {
		add(fmt.Sprintf("resolve value %d", i), "v: "+s+"\n")
		add(fmt.Sprintf("resolve key %d", i), s+": v\n")
		add(fmt.Sprintf("resolve nested key %d", i), "m:\n  "+s+": v\n")
		add(fmt.Sprintf("resolve sequence %d", i), "s:\n- "+s+"\n")
	}
	for i, tag := range scalarTags {
		for j, s := range taggedScalars {
			add(fmt.Sprintf("tag %d plain %d", i, j), "v: "+tag+" "+s+"\n")
			add(fmt.Sprintf("tag %d double %d", i, j), "v: "+tag+" \""+s+"\"\n")
			add(fmt.Sprintf("tag %d single %d", i, j), "v: "+tag+" '"+s+"'\n")
			add(fmt.Sprintf("tag %d key %d", i, j), tag+" "+s+": v\n")
			add(fmt.Sprintf("tag %d nested key %d", i, j), "m: {"+tag+" "+s+": v}\n")
		}
		add(fmt.Sprintf("tag %d sequence", i), "v: "+tag+" [1, a]\n")
		add(fmt.Sprintf("tag %d mapping", i), "v: "+tag+" {a: 1}\n")
		add(fmt.Sprintf("tag %d block mapping", i), "v: "+tag+"\n  a: 1\n")
		add(fmt.Sprintf("tag %d root mapping", i), tag+"\na: 1\n")
		add(fmt.Sprintf("tag %d root flow mapping", i), tag+" {a: 1}\n")
		add(fmt.Sprintf("tag %d document", i), "--- "+tag+"\na: 1\n")
		add(fmt.Sprintf("tag %d empty", i), "v: "+tag+"\nw: 1\n")
		add(fmt.Sprintf("tag %d mapping key", i), "m: {"+tag+" {a: 1}: v}\n")
		add(fmt.Sprintf("tag %d root mapping key", i), "? "+tag+" {a: 1}\n: v\n")
	}
	add("tag directive int", "%TAG !e! tag:yaml.org,2002:\n---\nv: !e!int '1'\n")
	add("tag directive local", "%TAG ! tag:example.com,2000:\n---\nv: !foo 1\n")
	add("tag directive secondary", "%TAG !! tag:example.com,2000:\n---\nv: !!int 1\n")
	add("tag directive undefined handle", "v: !e!x 1\n")
	add("tag directive twice", "%TAG !e! a\n%TAG !e! b\n---\nv: 1\n")
	add("tag directive escaped handle", "%TAG !%79! tag:yaml.org,2002:\n---\nv: !%79!int '1'\n")
	add("tag directive yaml prefix", "%TAG !y! tag:yaml.org,2002:\n---\nv: !y!binary aGk=\n")
	add("tag verbatim empty", "v: !<> x\n")
	add("tag verbatim unclosed", "v: !<tag:x x\n")
	add("tag uri escape bad", "v: !!%zz x\n")
	add("tag uri escape utf8", "v: !!%C3%A9 x\n")
	add("tag uri escape bad utf8", "v: !!%C3 x\n")
	add("tag then anchor", "a: !!str &x 1\nb: *x\n")
	add("anchor then tag", "a: &x !!str 1\nb: *x\n")
	add("two tags", "a: !!str !!int 1\n")

	binaries := []string{
		"aGVsbG8=", "aGVsbG8", "aGVsbG8==", "''", "\"\"", "=", "==", "/w==", "gA==", "4pyT", "YQ", "YQ=", "YR==", "YQ==",
		"\"aGVs\\r\\nbG8=\"", "\"aGVs\\tbG8=\"", "'aGVs bG8='", "|\n  aGVs\n  bG8=\n", ">\n  aGVs\n  bG8=\n",
		"\"aGVs\\nbG8=\"", "aGVs\n  bG8=", "\"aGVsbG8=\\n\"", "\"\\naGVsbG8=\"", "Zm9v\n  YmFy", "Zm9vYmFy", "Zm9v=YmFy", "!!!!", "YWJj\r\n  ZA==",
		"8J+Yg+KCrA==", "/+8=", "AA==", "AAA=", "AAAA",
	}
	for i, s := range binaries {
		add(fmt.Sprintf("binary %d", i), "v: !!binary "+s+"\n")
	}
	add("binary root key", "!!binary aGk=: v\n")
	add("binary root key not utf8", "!!binary /w==: v\n")
	add("binary nested key", "m: {!!binary aGk=: v}\n")
	add("binary nested string map", "m: {!!str a: 1, !!binary aGk=: v}\n")
	add("binary in sequence", "s: [!!binary /w==, !!binary aGk=]\n")
	add("binary invalid key", "!!binary ==: v\n")

	keys := []string{
		"1", "1.5", "0x10", "true", "yes", "~", "null", "", "2001-01-01", "2001-01-01T00:00:00Z", ".inf", ".nan",
		"-.inf", "-0", "-0.0", "9223372036854775808", "[1]", "[1, 2]", "[]", "{a: 1}", "{}", "\"1\"", "'1'", "!!str 1",
		"!!int 1", "!!float 1", "!!null x", "&k a", "*k", "\"\"", "<<", "'<<'",
	}
	for i, k := range keys {
		add(fmt.Sprintf("key root %d", i), k+": v\n")
		add(fmt.Sprintf("key root explicit %d", i), "? "+k+"\n: v\n")
		add(fmt.Sprintf("key nested %d", i), "m:\n  "+k+": v\n")
		add(fmt.Sprintf("key flow %d", i), "m: {"+k+": v}\n")
		add(fmt.Sprintf("key flow second %d", i), "m: {b: c, "+k+": v}\n")
		add(fmt.Sprintf("key sequence %d", i), "s:\n- "+k+": v\n")
		add(fmt.Sprintf("key anchored %d", i), "a: &k x\n"+k+": v\n")
	}
	rootSeqKey := "? - 1\n  - 2\n: v\n"
	add("key root block sequence", rootSeqKey)
	add("key root block mapping", "? a: 1\n  b: 2\n: v\n")
	add("key root properties on line before", "? &a\n  x: 1\n: v\n")
	add("key root two sequences", "? [1]\n: a\n? [2]\n: b\n")
	add("key nested two sequences", "m: {[1]: a, [2]: b}\n")
	add("key nested two mappings", "m: {{a: 1}: x, {b: 2}: y}\n")
	add("key nested invalid list", "m: {[1, 'a', 1.5, true, ~, [2], {b: c}, 2001-01-01, 9223372036854775808, .inf, -.inf, .nan, -0.0, 1e21, 1e20, 0.0001, 0.00001, \"q\\\"\\n\\t\\x01é\\u2028\"]: x}\n")
	add("key nested invalid map", "m: {{b: 1, a: [1]}: x}\n")
	add("key nested invalid general map", "m: {{1: a}: x}\n")
	add("key nested invalid general map 2", "m: {{2: a, 1: b, -1: c}: x}\n")
	add("key nested invalid map with time", "m: {{a: 2001-01-01T10:00:00+01:00, b: !!timestamp 2001-01-01}: x}\n")
	add("key nested invalid binary", "m: {[!!binary /w==]: x}\n")
	add("key nested invalid nested", "m: {[[[]], {}, [{}]]: x}\n")
	add("key nested invalid uint", "m: {[18446744073709551615, 0x8000000000000000]: x}\n")
	add("key nested invalid floats", "m: {[1.0, 100000.0, 1000000.0, 123456789.0, 0.000123, 1.5e-7, 5e-324, 1.7976931348623157e308]: x}\n")
	add("key nested invalid mixed map", "m: {{1: a, b: c, ~: d}: x}\n")
	add("key alias to sequence root", "a: &s [1]\n*s : v\n")
	add("key alias to sequence nested", "a: &s [1]\nm: {*s : v}\n")
	add("key alias to mapping nested", "a: &s {x: 1}\nm: {*s : v}\n")
	add("key alias to scalar", "a: &s 1\nm: {*s : v}\n")
	add("key alias to null", "a: &s\nm: {*s : v}\n")

	dups := []string{
		"a: 1\na: 2\n", "a: 1\nb: 2\na: 3\nb: 4\n", "m: {a: 1, a: 2}\n", "m:\n  a: 1\n  a: 2\nn: {b: 1, b: 2}\n",
		"a: 1\n'a': 2\n", "a: 1\n\"a\": 2\n", "1: a\n'1': b\n", "1: a\n01: b\n", "m: {1: a, 01: b}\n",
		"m: {1: a, 1.0: b}\n", "m: {.nan: a, .NaN: b}\n", "m: {.nan: a, .nan: b}\n", "m: {0: a, -0: b}\n",
		"m: {0.0: a, -0.0: b}\n", "m: {-0.0: a, 0.0: b}\n", "m: {true: a, True: b}\n", "m: {~: a, null: b}\n",
		"m: {1: a, +1: b, 0x1: c, 0o1: d, 0b1: e, 1_: f}\n", "m: {9223372036854775808: a, 0x8000000000000000: b}\n",
		"m: {!!timestamp 2001-01-01: a, !!timestamp 2001-1-1: b}\n",
		"m: {2001-01-01T00:00:00+01:00: a, 2001-1-1T0:0:0+01:00: b}\n",
		"m: {2001-01-01T00:00:00Z: a, 2001-1-1T0:0:0Z: b}\n",
		"&a x: 1\n*a : 2\n", "&a k: 1\n*a : 2\n*a : 3\n", "<<: {a: 1}\n<<: {b: 2}\n",
		"a: {x: 1, x: 2}\nb: *z\n", "a: [{x: 1, x: 2}]\n", "a: {x: 1, x: 2}\na: {y: 1, y: 2}\n",
		"a: {x: {y: 1, y: 2}}\nb: {z: 1, z: 2}\n", "a: 1\nb: 2\nc: 3\nb: 4\na: 5\n",
		"? [1]\n: a\nb: c\n? [1]\n: d\n", "\"\\u00e9\": 1\né: 2\n", "\"a\\tb\": 1\n\"a\\tb\": 2\n",
		"? |\n  x\n: 1\n? x\n: 2\n", "x: 1\n? |-\n  x\n: 2\n", "m: {a: 1, b: {a: 1}, a: 2}\n",
		"m: [{a: 1, a: 2}, {b: 1, b: 2}]\n", "{a: 1, a: 2}\n", "[{a: 1, a: 2}]\n",
	}
	for i, d := range dups {
		add(fmt.Sprintf("duplicate %d", i), d)
	}

	merges := []string{
		"a: &a {x: 1, y: 2}\nb:\n  <<: *a\n  y: 3\n",
		"a: &a {x: 1}\nb: &b {y: 2}\nc:\n  <<: [*a, *b]\n  z: 3\n",
		"<<: {a: 1, b: 2}\nb: 3\n",
		"x: &x {1: a, 2: b}\n<<: *x\n1: c\n",
		"x: &x {1: a}\ny:\n  <<: *x\n  1: c\n",
		"x: &x {a: 1}\ny:\n  <<: *x\n  1: c\n",
		"y:\n  <<: {1: a}\n  b: c\n",
		"a: {<<: 1}\n", "a: {<<: [1]}\n", "a: &a [1]\nb: {<<: *a}\n", "a: &a 1\nb: {<<: [*a]}\n",
		"a: {<<: ~}\n", "a: {<<: }\n", "a: {<<: {}}\n", "a: {<<: []}\n", "a: {'<<': {x: 1}}\n", "a: {\"<<\": {x: 1}}\n",
		"a: {!!merge <<: {x: 1}}\n", "a: {! <<: {x: 1}}\n", "a: {!!str <<: {x: 1}}\n",
		"a: {!<tag:yaml.org,2002:merge> <<: {x: 1}}\n", "a: {!!merge x: {y: 1}}\n", "a: {<<: {x: 1}, <<: {y: 2}}\n",
		"a: &a {<<: {x: 1}, y: 2}\nb: {<<: *a, z: 3}\n", "a: &a {x: 1}\nb: {<<: [*a, *a]}\n",
		"a: {<<: {[1]: x}}\n", "a: {<<: {[1]: x}, 1: y}\n", "a: {<<: {{b: 1}: x}, 1: y}\n", "a: {1: y, <<: {[1]: x}}\n",
		"[1]: a\n<<: {b: c}\n", "{x: 1}: a\n<<: {b: c}\n", "<<: *a\n", "a: &a {x: 1}\n<<: *a\nx: 2\n",
		"- <<: {a: 1}\n  b: 2\n", "<<: [{a: 1}, {a: 2}]\n", "a: {<<: [&x {a: 1}, *x]}\n",
		"a: &a {x: 1}\nb: {<<: *a, x: 2}\nc: {x: 2, <<: *a}\n", "a: {<<: {<<: {x: 1}, y: 2}, z: 3}\n",
		"a: &a {<<: {x: 1}}\nb: {<<: [*a, {x: 2, y: 3}], y: 4}\n", "<<: {a: {b: 1}}\na: {c: 2}\n",
		"a: {<<: [{x: 1}, [1]]}\n", "a: {<<: [{x: 1}, 1]}\n", "a: {<<: [{x: 1}, ~]}\n",
		"a: &a {x: 1}\nb: {<<: [*a, *z]}\n", "<<: {1: a, 1.5: b, true: c, ~: d}\n",
		"<<: {[1]: a}\n", "a: {<<: {x: 1, x: 2}}\n", "a: {<<: {x: 1}, x: 2, x: 3}\n",
		"<<: [*a]\n", "a: &a {b: &b {c: 1}}\nd: {<<: *b}\n", "a: {<<: {x: ~}}\n", "a: {x: ~, <<: {x: 1}}\n",
		"a: !!map {<<: {x: 1}}\n", "a: {<<: !!map {x: 1}}\n", "a: {<<: !!seq [{x: 1}]}\n", "a: {<<: !!str {x: 1}}\n",
		"a: &a {x: 1}\nb: {<<: !!str *a}\n", "a: {<<: &b {x: 1}, c: *b}\n",
		"a: &a {x: 1}\nb: &b {<<: *a}\nc: {<<: *b}\n", "a: &a [&b {x: 1}]\nc: {<<: *a}\n",
		"a:\n<<:\n-\n?\n-\n",
	}
	for i, d := range merges {
		add(fmt.Sprintf("merge %d", i), d)
	}

	aliases := []string{
		"a: &x 1\nb: *x\n", "a: &x\nb: *x\n", "a: &x [1, 2]\nb: *x\n", "a: &x {k: v}\nb: *x\n", "&x a: 1\nb: *x\n",
		"a: *x\n", "a: &x [*x]\n", "a: &x {k: *x}\n", "&x [*x]\n", "a: &x 1\na2: &x 2\nb: *x\n", "a: &x 1\nb: &y *x\n",
		"a: &k key\n*k : v\n", "a: &é x\nb: *é\n", "a: &a,b c\n", "a: [&a] b\n", "a: [&a b, *a]\n", "a: &a b\nc: [*a]\n",
		"a: *\n", "a: &\n", "a: & x\n", "a: [*a,]\n", "a: &x [&y [*x]]\n", "a: &x [&y [*y]]\n",
		"a: &x {b: &y [*x]}\n", "&x\na: *x\n", "--- &x\na: 1\n", "--- *x\n", "a: &x !!str\nb: *x\n",
		"a: &x ~\nb: {*x : 1}\n", "a: &x 1\nb: {*x : 1, *x : 2}\n", "a: &x\n  - 1\nb: *x\n",
		"a: &x |\n  text\nb: *x\n", "a: &x\n\n  y: 1\nb: *x\n", "a: &x &y 1\n",
	}
	for i, d := range aliases {
		add(fmt.Sprintf("alias %d", i), d)
	}
	// Alias expansion around the allowedAliasRatio limits.
	for _, n := range []int{2, 5, 9, 10, 11, 12} {
		var b strings.Builder
		b.WriteString("a: &a [00,00,00,00,00,00,00,00,00]\n")
		prev := "a"
		for i := 1; i < n; i++ {
			name := string(rune('a' + i))
			b.WriteString(name + ": &" + name + " [")
			for j := 0; j < 9; j++ {
				if j > 0 {
					b.WriteString(",")
				}
				b.WriteString("*" + prev)
			}
			b.WriteString("]\n")
			prev = name
		}
		add(fmt.Sprintf("alias laughs %d", n), b.String())
	}
	for _, n := range []int{100, 101, 200, 1000} {
		var b strings.Builder
		b.WriteString("a: &a [x]\nb: [")
		for j := 0; j < n; j++ {
			if j > 0 {
				b.WriteString(",")
			}
			b.WriteString("*a")
		}
		b.WriteString("]\n")
		add(fmt.Sprintf("alias many %d", n), b.String())
	}
	for _, n := range []int{10, 50, 100, 120} {
		var b strings.Builder
		b.WriteString("a: &a [")
		for j := 0; j < 1000; j++ {
			if j > 0 {
				b.WriteString(",")
			}
			b.WriteString("x")
		}
		b.WriteString("]\nb: [")
		for j := 0; j < n; j++ {
			if j > 0 {
				b.WriteString(",")
			}
			b.WriteString("*a")
		}
		b.WriteString("]\n")
		add(fmt.Sprintf("alias ratio %d", n), b.String())
	}

	docs := []string{
		"a: 1\n---\nb: 2\n", "---\na: 1\n...\n---\nb: 2\n", "a: 1\n...\n", "a: 1\n...\n@\n", "a: 1\n---\n@\n",
		"a: 1\n--- @\n", "a: 1\n---\n- [\n", "a: 1\n---\nb: *x\n", "---\n---\na: 1\n", "--- a\n--- b\n", "--- |\n  a\n",
		"a: 1\n... # c\n", "...\na: 1\n", "...\n", "--- ...\n", "---\n...\n", "%YAML 1.1\n---\na: 1\n", "%YAML 1.2\n---\na: 1\n",
		"%YAML 1.3\n---\na: 1\n", "%YAML 2.0\n---\na: 1\n", "%YAML 1.0\n---\na: 1\n", "%YAML 1.1\n%YAML 1.1\n---\na: 1\n",
		"%YAML 1.1\na: 1\n", "%YAML 1\n---\na: 1\n", "%YAML 1.1.1\n---\na: 1\n", "%YAML 1.x\n---\na: 1\n",
		"%YAML 123456789.1\n---\na: 1\n", "%FOO bar\n---\na: 1\n", "%\n---\na: 1\n", "%YAML 1.1 # c\n---\na: 1\n",
		"%YAML 1.1 x\n---\na: 1\n", "%TAG !e!\n---\na: 1\n", "%TAG !e! x y\n---\na: 1\n",
		"%YAML 1.1\n---\na: 1\n...\n%YAML 1.1\n---\nb: 2\n", "a: 1\n%YAML 1.1\n---\nb: 2\n", "a: 1\n...\n%YAML 1.1\n",
		"a: 1\n...\n---\n", "a: 1\n... a\n", "a: 1\n...a\n", "a: 1\n---a\n", "--- a: 1\n", "---a: 1\n", "--- \n  a: 1\n",
		"---\n  a: 1\n b: 2\n", "a: 1\n--- \nb: 2\n...\n--- [\n", "a: 1\n...\n}not yaml\n", "a: 1\n...\n\"\n",
		"a: 1\n...\n'\n", "a: 1\n...\n\t\n", "a: 1\n...\n- - - @\n", "a: 1\n---\n---\n---\n", "a: 1\n--- !x\n",
		"a: 1\n--- &x\n", "a: 1\n--- *x\n", "a: 1\n---\n%YAML 1.1\n", "a\n...\n@\n", "a: [1]\n...\n[\n",
		"a: 1\n...\nb: [\n", "a: 1\n...\n? [\n", "a: 1\n...\nx\ny: [\n", "a: 1\n... \n# c\n@\n",
	}
	for i, d := range docs {
		add(fmt.Sprintf("documents %d", i), d)
	}

	roots := []string{
		"", " ", "\n", "\n\n", "# c", "# c\n", "---", "--- # c", "~", "null", "1", "a", "'a'", "[1]", "- 1", "{}", "[]",
		"{a: 1}", "!!map {}", "--- !!str", "--- !!null", "&a ~", "\"\"", "|\n  x\n", "? a\n: b\n", "? a\n", "? [a]\n: b\n",
		"a: 1", "a:", "a", "a:b", "a :b", "a : b", "true", "2001-01-01", "!!binary aGk=", "*a", "&a a: 1", "!!null",
		"--- !!map\n", "--- !!seq\n", "!!str\na: 1\n", "!!seq\n- 1\n", "!!null\na: 1\n", "--- !!null {a: 1}\n",
		"!!int {a: 1}\n", "- a: 1\n", "[a: 1]\n", "? \n", "?\n", ":\n", ": a\n", "-\n", "- \n", "{\n}\n", "[\n]\n",
	}
	for i, d := range roots {
		add(fmt.Sprintf("root %d", i), d)
	}

	strs := []string{
		"v: \"\\x41\\u00e9\\U0001F600\\n\\t\\0\\a\\b\\e\\f\\r\\v\\/\\ \\_\\N\\L\\P\\\"\\\\\"\n", "v: \"\\q\"\n", "v: \"\\x4\"\n",
		"v: \"\\x4g\"\n", "v: \"\\uD800\"\n", "v: \"\\uDFFF\"\n", "v: \"\\U00110000\"\n", "v: \"\\U0010FFFF\"\n", "v: \"\\u0000\"\n",
		"v: \"\\x00\"\n", "v: \"\\xff\"\n", "v: \"\\t\"\n", "v: \"\\\t\"\n", "v: \"a\\\n  b\"\n", "v: \"a\n  b\"\n", "v: \"a\n\n  b\"\n",
		"v: \"a  \n  b\"\n", "v: \"a\\ \n  b\"\n", "v: \"unterminated\n", "v: \"a\n---\nb\"\n", "v: \"a\n...\nb\"\n", "v: 'it''s'\n",
		"v: 'a\n  b'\n", "v: '\\n'\n", "v: 'unterminated\n", "v: 'a\n\n\n  b'\n", "v: \"\"\n", "v: ''\n", "v: '''\n", "v: \"\\\"\n",
		"v: \"a\"b\n", "v: \"a\" b\n", "v: 'a'b\n", "v: \"a\" # c\n", "v: \"a\"#c\n", "v: \"a\":b\n", "\"a\":b\n", "\"a\" : b\n",
		"v: |\n  a\n  b\n", "v: |-\n  a\n", "v: |+\n  a\n\n", "v: >\n  a\n  b\n\n  c\n", "v: >-\n  a\n", "v: |2\n   a\n",
		"v: |0\n  a\n", "v: |\n\ta\n", "v: |\n  a\n\tb\n", "v: |\n  a\n b\n", "v: >\n  a\n   b\n  c\n", "v: |\n", "v: |",
		"v: | # c\n  a\n", "v: |x\n  a\n", "v: |1-\n  a\n", "v: |-1\n  a\n", "v: >+\n\n", "v: |\n  \n  a\n", "- |\n  a\n- b\n",
		"v: |\n   a\n  b\n", "v: |10\n          a\n", "v: |\n  a\n  \n", "v: >\n\n  a\n", "v: >\n  a\n\n\n", "v: >2-\n   a\n   b\n",
		"v: |\n  a\n#c\n", "v: |\n  a\n  # c\n", "v: >\n a\n\tb\n", "v: |#c\n  a\n", "v: |-+\n  a\n", "v: |99\n  a\n",
		"v: a\n  b\n  c\n", "v: a\n\n  b\n", "v: a #c\n  b\n", "v: a:b\n", "v: a: b\n", "v: a #b\n", "v: a#b\n", "v: -a\n",
		"v: ?a\n", "v: :a\n", "v: a,b\n", "v: [a,b]\n", "v: {a,b}\n", "v: [a:b]\n", "v: @a\n", "v: `a\n", "v: %a\n", "v: !a\n",
		"v: &a\n", "v: a\tb\n", "v:\ta\n", "v: a\t\n", "v :a\n", "v:a\n", "- a\n-b\n", "-\ta\n", "- a\n- - b\n  - c\n",
		"v:\n- a\n- b\n", "v:\n  - a\n  -b\n", "? a\n? b\n", "a:\n  b:\n c: 1\n", "a: 1\n b: 2\n", "a: 1\n  b: 2\n",
		"\ta: 1\n", "a:\n\tb: 1\n", " a: 1\nb: 2\n", "a: [1, 2\n", "a: {b: 1\n", "a: [1, 2]]\n", "a: {b: 1}}\n",
		"a: [1,, 2]\n", "a: {,}\n", "a: [,]\n", "a: {b}\n", "a: {b: }\n", "a: {: c}\n", "a: [b: c]\n", "a: [? b : c]\n",
		"a: [b: c, d]\n", "a: {b: c, d: [e, {f: g}]}\n", "a: [1\n,2]\n", "a: [\n1,\n2\n]\n", "a: {\nb: 1\n}\n", "a: [\n  - 1]\n",
		"[a]: b\n", "{a: b}: c\n", "a: !!str\nb: 1\n", "a: [b, c]: d\n", "a: {b: c}: d\n", "a: [a\n  b]\n", "a: [a\nb]\n",
		"a: {a\nb: c}\n", "a: {\"a\"\n: c}\n", "a: [\"a\"\n: c]\n", "a: [a: b: c]\n", "a: [a, b]x\n", "a: {a: b}x\n",
		"- [a, b]: c\n", "- {a: b}: c\n", "a: -\n", "a: - b\n", "a: ? b\n", "a: : b\n", "a: [- b]\n", "a: [? b]\n",
		"a: [?]\n", "a: [:]\n", "a: {?}\n", "a: {:}\n", "a: [a:]\n", "a: [:a]\n", "a: [::]\n", "a: [-]\n", "a: [-a]\n",
		"a: [a]]\n", "a: ]\n", "a: }\n", "]\n", "}\n", ",\n", "a: ,\n", "a: b, c\n", "a: [b]c\n",
		"a: 1 # c\n", "a: 1# c\n", "# c\na: 1\n", "a:\n  # c\n  b: 1\n", "a: 1\n# c\n  b: 2\n", "a: |\n  x\n # c\n",
		"a: [1, # c\n 2]\n", "a: 1 #c\n#d\nb: 2 #e\n", "#c\n#d\n", "a: #c\n  b\n", "a: # c\n  - 1\n", "- #c\n  a\n",
		"a: 1\r\nb: 2\r\n", "a: 1\rb: 2\r", "a: |\r\n  x\r\n  y\r\n", "a: \"x\r\n  y\"\r\n", "a: x\r\n  y\r\n",
		"é: ü\n", "a: 日本語\n", "a: \U0001F600\n", "a: b\u2028c\n", "a: b\u2029c\n", "a: b\u0085c\n", "a: b\n\u2028c: d\n",
		"\u2028a: 1\n", "a: \"b\u2028c\"\n", "a: |\n  b\u2028c\n", "a: b\u2028\n  c\n", "a: 1\u2028b: 2\n", "a: 1\u0085b: 2\n",
		"a: '\u0085'\n", "a: \"\\u2028\"\n", "a: \u00a0\n", "a:\u00a0b\n", "a: \u3000\n",
		"\ufeffa: 1\n", "\ufeff\ufeffa: 1\n", "a: 1\n\ufeffb: 2\n", "a: \ufeff1\n", "\ufeff", "\ufeff---\na: 1\n",
		"a: 1\n\ufeff---\nb: 2\n", "- \ufeff\n", "a: [\ufeff]\n", "a: \"\ufeff\"\n", " \ufeff\n", "? \ufeff\n", "? \ufeff:\n",
		"0: \ufeff\n", "? \ufeff: \ufeff\n",
	}
	for i, d := range strs {
		add(fmt.Sprintf("syntax %d", i), d)
	}

	enc := []string{
		utf16le("a: 1\n", true), utf16be("a: 1\n", true), utf16le("é: \U0001F600\n", true), utf16be("é: \U0001F600\n", true),
		utf16le("a: 1\n", false), utf16be("a: 1\n", false), "\xff\xfe", "\xfe\xff", "\xff\xfea", "\xfe\xff\x00",
		"\xff\xfe\x00\xd8a\x00", "\xff\xfe\x00\xdc", "\xff\xfe\x00\xd8", "\xff\xfe\x00\xd8\x00", "\xfe\xff\xd8\x00\x00a",
		"\xfe\xff\xdc\x00", "\xff\xfea\x00:\x00 \x00\x00\x00", "\xef\xbb\xbf", "\xef\xbb", "\xef\xbbx", "\xef\xbb\xbf\xef\xbb\xbfa: 1",
		"a: \xff\n", "a: \xc3\n", "a: \xc3(\n", "a: \xe2\x82\n", "a: \xed\xa0\x80\n", "a: \xf4\x90\x80\x80\n", "a: \xc0\x80\n",
		"a: \xf8\x88\x80\x80\x80\n", "a: \x80\n", "a: \xe0\x80\x80\n", "a: \xf0\x80\x80\x80\n", "a: \xc3", "a: \xf0\x9f\x98",
		"a: \x00\n", "a: \x01\n", "a: \x7f\n", "a: \x1b\n", "a: \u0080\n", "a: \u009f\n", "a: \ufffe\n", "a: \uffff\n",
		"a: \U000F0000\n", "a: \U0010FFFF\n", "\x00", "a: 1\n\x00", "# \x01\na: 1\n", "a: \"\x01\"\n", "a: \x0b\n", "a: \x0c\n",
		"a: 1\n...\n\xff\n", "a: 1\n...\n\x00\n", utf16le("a: 1\n...\n", true) + "\x00", "0: [:!00 \xef",
	}
	for i, d := range enc {
		add(fmt.Sprintf("encoding %d", i), d)
	}
	// Encoding errors beyond the first 512-byte chunk the reader decodes.
	long := strings.Repeat("x", 600)
	add("encoding late in scalar", "a: "+long+"\xff\n")
	add("encoding late after document", "a: 1\n...\n#"+long+"\xff\n")
	add("encoding late in comment", "a: 1\n#"+long+"\xff\nb: 2\n")
	add("encoding late control", "a: "+long+"\x01\n")
	add("encoding late second document", "a: 1\n---\nb: "+long+"\xff\n")
	add("encoding late after scalar line", "a: b\n#"+long+"\xffc\n")
	add("encoding split at chunk", strings.Repeat("x", 509)+": \u00e9\u00e9\u00e9\n")
	add("encoding split at chunk 2", "a: "+strings.Repeat("x", 508)+"\u20ac\u20ac\n")
	add("encoding split invalid at chunk", "a: "+strings.Repeat("x", 508)+"\xe2\x82(\n")
	add("encoding late utf16", utf16le("a: "+long+"\n", true)+"\x00\xdc")
	add("encoding late utf16 odd", utf16le("a: "+long+"\n", true)+"a")
	add("encoding late flow error first", "a: [\n#"+long+"\xff\n")
	add("encoding late after flow", "a: [1, 2]\n"+strings.Repeat("#"+strings.Repeat("y", 60)+"\n", 9)+"\xff")
	// Simple keys up to and past 1024 characters.
	add("simple key 1023", strings.Repeat("k", 1023)+": v\n")
	add("simple key 1024", strings.Repeat("k", 1024)+": v\n")
	add("simple key 1025", strings.Repeat("k", 1025)+": v\n")
	add("simple key 1100", strings.Repeat("k", 1100)+": v\n")
	add("simple key flow 1100", "{"+strings.Repeat("k", 1100)+": v}\n")
	add("simple key quoted 1100", "\""+strings.Repeat("k", 1100)+"\": v\n")
	// Nesting depth.
	add("depth flow 10000", strings.Repeat("[", 10000)+strings.Repeat("]", 10000)+"\n")
	add("depth flow 10001", strings.Repeat("[", 10001)+strings.Repeat("]", 10001)+"\n")
	add("depth flow map 10000", "a: "+strings.Repeat("{a: ", 9999)+"b"+strings.Repeat("}", 9999)+"\n")
	add("depth block 9999", strings.Repeat("- ", 9999)+"a\n")
	add("depth block 10000", strings.Repeat("- ", 10000)+"a\n")
	add("depth block 10001", strings.Repeat("- ", 10001)+"a\n")
	var deep strings.Builder
	for i := 0; i < 300; i++ {
		deep.WriteString(strings.Repeat(" ", i) + "k:\n")
	}
	add("depth block map 300", deep.String())
	add("depth mixed", strings.Repeat("- ", 5000)+strings.Repeat("[", 5000)+strings.Repeat("]", 5000)+"\n")
	return out
}

// localCases are timestamps decoded with time.Local = FixedZone("CET",
// 3600).
func localCases() []namedInput {
	var out []namedInput
	for i, s := range []string{
		"a: 2001-01-01T10:00:00+01:00\n", "a: 2001-01-01T10:00:00+02:00\n", "a: 2001-01-01T10:00:00Z\n",
		"a: 2001-01-01T10:00:00+00:00\n", "a: 2001-01-01\n", "a: 2001-01-01 10:00:00\n", "a: !!timestamp 2001-01-01T10:00:00+01:00\n",
		"m: {2001-01-01T00:00:00+01:00: a, 2001-1-1T0:0:0+01:00: b}\n", "m: {[2001-01-01T00:00:00+01:00, 2001-01-01T00:00:00+02:00]: a}\n",
	} {
		out = append(out, namedInput{fmt.Sprintf("local %d", i), s})
	}
	return out
}

// mutationInserts are what a mutation inserts or writes over a byte.
var mutationInserts = []string{
	"-", "?", ":", ",", "[", "]", "{", "}", "#", "&", "*", "!", "|", ">", "'", "\"", "%", "@", "`", "~", ".", " ", " ",
	"\t", "\n", "\n", "\r", "0", "1", "9", "a", "x", "N", "n", "t", "T", "y", "_", "+", "\\", "e", "<",
	"- ", ": ", "\n- ", "\n  ", "---\n", "...\n", "<<: ", "*a", "&a ", "!!str ", "!!int ", "!!binary ", "? ", "\xc2\x85",
	"\xe2\x80\xa8", "\xef\xbb\xbf", "\xff", "\x00", "\x01", "é",
}

// mutations adds seeded mutations of the inputs that decode without error.
func mutations(c *corpus) {
	var seeds [][]byte
	for i, k := range c.cases {
		if k.Error != "" || k.Panic != "" || c.inputs[i] == nil || len(c.inputs[i]) == 0 || len(c.inputs[i]) > 8192 {
			continue
		}
		seeds = append(seeds, c.inputs[i])
	}
	r := rand.New(rand.NewSource(20261019))
	added := 0
	for attempt := 0; added < 6000 && attempt < 60000; attempt++ {
		seed := seeds[r.Intn(len(seeds))]
		if len(seed) > 2048 && r.Intn(8) != 0 {
			continue
		}
		b := append([]byte(nil), seed...)
		for n := 1 + r.Intn(3); n > 0; n-- {
			pos := r.Intn(len(b) + 1)
			switch r.Intn(3) {
			case 0:
				ins := mutationInserts[r.Intn(len(mutationInserts))]
				b = append(b[:pos], append([]byte(ins), b[pos:]...)...)
			case 1:
				if pos < len(b) {
					b = append(b[:pos], b[pos+1:]...)
				}
			case 2:
				if pos < len(b) {
					ins := mutationInserts[r.Intn(len(mutationInserts))]
					b = append(b[:pos], append([]byte(ins), b[pos+1:]...)...)
				}
			}
		}
		if c.add(fmt.Sprintf("mutation %d", added), b) != nil {
			added++
		}
	}
}
