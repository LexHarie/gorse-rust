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

// Command gobgolden writes golden data produced by common/encoding and
// encoding/gob for the Rust port's tests:
//
//   - -out: WriteSlice, WriteString, WriteBytes, and WriteGob of every Go type
//     that Gorse gob-encodes (contracts/model-blobs.md);
//   - -decode-out: what Go's gob decoder makes of hand-made streams, valid and
//     corrupt, decoded into those types: the value, or the error text.
//
// gob assigns type ids process-wide in first-use order, so each encoding is
// made in a fresh child process; the ids then always start at 64, which is
// what a fresh Rust encoder does too.
package main

import (
	"bytes"
	"encoding/gob"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"os/exec"
	"sort"
	"strconv"

	"github.com/gorse-io/gorse/common/encoding"
	"github.com/gorse-io/gorse/model"
)

// Case is one golden value. Value describes it in a form both sides can read
// exactly: integers as decimal strings, floats as their IEEE bits in hex.
type Case struct {
	Name          string `json:"name"`
	Kind          string `json:"kind"`
	Value         any    `json:"value"`
	Hex           string `json:"hex"`
	Deterministic bool   `json:"deterministic"`
}

// Entry describes one model.Params entry: key, Go dynamic type, value.
type Entry struct {
	Key   string `json:"key"`
	Type  string `json:"type"`
	Value any    `json:"value"`
}

type spec struct {
	name, kind    string
	value         any
	describe      any
	deterministic bool
	write         func(w *bytes.Buffer) error
}

func gobSpec(name, kind string, v, describe any) spec {
	return spec{name: name, kind: kind, value: v, describe: describe, deterministic: true,
		write: func(w *bytes.Buffer) error { return encoding.WriteGob(w, v) }}
}

func f32(v float32) string { return fmt.Sprintf("%08x", math.Float32bits(v)) }
func f64(v float64) string { return fmt.Sprintf("%016x", math.Float64bits(v)) }

func describeParams(p model.Params) []Entry {
	var entries []Entry
	for k, v := range p {
		var t string
		var d any
		switch v := v.(type) {
		case int:
			t, d = "int", strconv.Itoa(v)
		case int64:
			t, d = "int64", strconv.FormatInt(v, 10)
		case int32:
			t, d = "int32", strconv.FormatInt(int64(v), 10)
		case float32:
			t, d = "float32", f32(v)
		case float64:
			t, d = "float64", f64(v)
		case bool:
			t, d = "bool", v
		case string:
			t, d = "string", v
		case []int:
			t = "[]int"
			var s []string
			for _, x := range v {
				s = append(s, strconv.Itoa(x))
			}
			d = s
		case nil:
			t, d = "nil", nil
		default:
			t, d = fmt.Sprintf("%T", v), fmt.Sprint(v)
		}
		entries = append(entries, Entry{Key: string(k), Type: t, Value: d})
	}
	// Sorted so that the file does not depend on map order.
	sort.Slice(entries, func(i, j int) bool { return entries[i].Key < entries[j].Key })
	return entries
}

func params(name string, p model.Params, deterministic bool) spec {
	s := gobSpec(name, "params", p, describeParams(p))
	s.deterministic = deterministic
	return s
}

func specs() []spec {
	var s []spec
	for _, v := range []int64{0, 1, -1, 3, 63, 64, -64, -65, 127, 128, 255, 256, 1 << 40, math.MaxInt64, math.MinInt64} {
		s = append(s, gobSpec("int64 "+strconv.FormatInt(v, 10), "int64", v, strconv.FormatInt(v, 10)))
	}
	for _, v := range []int{0, 3, -1, 1024, math.MaxInt} {
		s = append(s, gobSpec("int "+strconv.Itoa(v), "int", v, strconv.Itoa(v)))
	}
	for _, v := range []int32{0, 7, -7, math.MaxInt32, math.MinInt32} {
		s = append(s, gobSpec("int32 "+strconv.Itoa(int(v)), "int32", v, strconv.Itoa(int(v))))
	}
	s = append(s, gobSpec("bool true", "bool", true, true), gobSpec("bool false", "bool", false, false))
	for _, v := range []float32{0, 0.5, -1.25, 1, 3.4028235e38, 1e-45, float32(math.Inf(1)), float32(math.Inf(-1)), 0.1} {
		s = append(s, gobSpec("float32 "+f32(v), "float32", v, f32(v)))
	}
	for _, v := range []float64{0, 0.125, -2.5, 1e300, 0.1} {
		s = append(s, gobSpec("float64 "+f64(v), "float64", v, f64(v)))
	}
	for _, v := range []string{"", "abc", "héllo", "AFM2"} {
		s = append(s, gobSpec("string "+strconv.Quote(v), "string", v, v))
	}
	for i, v := range [][]int{{}, {1, 2}, {64, 32}, {-5, 300, 1 << 33}} {
		var d []string
		for _, x := range v {
			d = append(d, strconv.Itoa(x))
		}
		s = append(s, gobSpec("[]int "+strconv.Itoa(i), "[]int", v, d))
	}
	s = append(s,
		params("params empty", model.Params{}, true),
		params("params nil", model.Params(nil), true),
		params("params float32", model.Params{model.Lr: float32(0.5)}, true),
		params("params float64", model.Params{model.Reg: 0.1}, true),
		params("params int", model.Params{model.NEpochs: 3}, true),
		params("params int64", model.Params{model.RandomState: int64(7)}, true),
		params("params int32", model.Params{model.BatchSize: int32(9)}, true),
		params("params bool", model.Params{model.AutoScale: true}, true),
		params("params string", model.Params{model.Optimizer: model.Adam}, true),
		params("params []int", model.Params{model.HiddenLayers: []int{64, 32}}, true),
		params("params nil value", model.Params{model.Similarity: nil}, true),
		params("params zero int", model.Params{model.NFactors: 0}, true),
		params("params all", model.Params{
			model.Lr: float32(0.05), model.Reg: 0.01, model.NEpochs: 100, model.NFactors: 16,
			model.RandomState: int64(1), model.AutoScale: false, model.Optimizer: model.SGD,
			model.HiddenLayers: []int{8}, model.InitStdDev: float32(0.001),
		}, false),
	)
	return s
}

func run(i int) string {
	sp := specs()[i]
	var b bytes.Buffer
	if err := sp.write(&b); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b.Bytes())
}

// renumbered encodes model.Params after other types have taken the first
// type ids, as in a long-running Go process.
func renumbered() string {
	var b bytes.Buffer
	if err := encoding.WriteGob(&b, struct{ A []string }{}); err != nil {
		panic(err)
	}
	if err := encoding.WriteGob(&b, []float64{1}); err != nil {
		panic(err)
	}
	b.Reset()
	enc := gob.NewEncoder(&b)
	if err := enc.Encode(model.Params{model.HiddenLayers: []int{1, 2}}); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b.Bytes())
}

// DecodeCase is a gob stream (without the WriteGob length prefix) and what
// Go's decoder returns for it in the target type: Value when OK, else Error.
type DecodeCase struct {
	Name   string `json:"name"`
	Target string `json:"target"`
	Hex    string `json:"hex"`
	OK     bool   `json:"ok"`
	Error  string `json:"error"`
	Value  any    `json:"value"`
}

const (
	intSliceDef = "0b7f020102ff800001040000"                     // -64: []int
	paramsDef   = "157f04010106506172616d7301ff8000010c01100000" // -64: Params
)

const rustParams = "157f04010106506172616d7301ff8000010c011000004aff80000b094175746f5363616c6504626f6f6c0202000009426174636853697a6505696e743332040200110c48696464656e4c6179657273055b5d696e74ff81020102ff820001040000ffaaff82030001100a496e697453746444657607666c6f61743332080700fbe04d62503f024c7207666c6f61743332080700fba09999a93f074e45706f63687303696e74040300ffc8084e466163746f727303696e7404020020094f7074696d697a657206737472696e670c0500037367640b52616e646f6d537461746505696e743634040200020352656707666c6f61743634080a00f87b14ae47e17a843f0a53696d696c617269747900"

func decodeCases(renumberedHex string) []DecodeCase {
	cases := []DecodeCase{
		{Name: "empty stream", Target: "int64", Hex: ""},
		{Name: "type definition only", Target: "[]int", Hex: intSliceDef},
		{Name: "trailing bytes", Target: "int64", Hex: "05040006aabb01cc"},
		{Name: "truncated message", Target: "int64", Hex: "05040006"},
		{Name: "zero-length message", Target: "int64", Hex: "0003040006"},
		{Name: "message count too big", Target: "int64", Hex: "fb0200000000"},
		{Name: "nine-byte uint", Target: "int64", Hex: "f7010203040506070809"},
		{Name: "non-zero singleton delta", Target: "int64", Hex: "03040106"},
		{Name: "int32 overflow", Target: "int32", Hex: "080400fb0100000000"},
		{Name: "float32 overflow", Target: "float32", Hex: "0b0800f89c7500883ce4377e"},
		{Name: "float32 infinity", Target: "float32", Hex: "050800fef07f"},
		{Name: "int into float64", Target: "float64", Hex: "03040006"},
		{Name: "float into int64", Target: "int64", Hex: "050800fef07f"},
		{Name: "bool from 2", Target: "bool", Hex: "03020002"},
		{Name: "string into int64", Target: "int64", Hex: "060c0003616263"},
		{Name: "invalid UTF-8 string", Target: "string", Hex: "050c0002fffe"},
		{Name: "string longer than message", Target: "string", Hex: "040c000561"},
		{Name: "[]int into params", Target: "params", Hex: intSliceDef + "06ff8000020204"},
		{Name: "[]int into int64", Target: "int64", Hex: intSliceDef + "06ff8000020204"},
		{Name: "params into []int", Target: "[]int", Hex: paramsDef + "04ff800000"},
		{Name: "duplicate type definition", Target: "[]int", Hex: intSliceDef + intSliceDef + "04ff800000"},
		{Name: "type id below 64", Target: "[]int", Hex: "0b3f020102ff800001040000"},
		{Name: "extra data after top-level type", Target: "[]int", Hex: "0c7f020102ff80000104000000" + "04ff800000"},
		{Name: "wire struct without terminator", Target: "[]int", Hex: "0a7f020102ff8000010400" + "06ff8000020204"},
		{Name: "wire field out of bounds", Target: "[]int", Hex: "027f08"},
		{Name: "slice marked as GobEncoder", Target: "[]int", Hex: "127f020102ff80000104000301" + "02ff80000000" + "04ff800000"},
		{Name: "slice length exceeds data", Target: "[]int", Hex: intSliceDef + "05ff80000302"},
		{Name: "slice too big", Target: "[]int", Hex: intSliceDef + "08ff8000fc40000001"},
		{Name: "defined but undescribed id", Target: "[]int", Hex: "04ff820000"},
		{Name: "params with uint", Target: "params", Hex: paramsDef + "0fff80000101" + "6b" + "0475696e74" + "06020001"},
		{Name: "params duplicate keys", Target: "params", Hex: paramsDef + "18ff800002" + "0161" + "03696e74" + "04020002" + "0161" + "03696e74" + "04020004"},
		{Name: "params unregistered name", Target: "params", Hex: paramsDef + "0aff80000101" + "6b" + "03666f6f"},
		{Name: "params interface id mismatch", Target: "params", Hex: paramsDef + "0eff80000101" + "6b" + "03696e74" + "08" + "020006"},
		{Name: "params count wraps negative", Target: "params", Hex: paramsDef + "0cff8000f88000000000000000"},
		{Name: "params entries past data", Target: "params", Hex: paramsDef + "04ff800002"},
		{Name: "params defined in separate messages", Target: "params", Hex: paramsDef + "0cff81020102ff820001040000" +
			"1fff8000010c48696464656e4c6179657273055b5d696e74ff82050002ff8040"},
		{Name: "params after other types", Target: "params", Hex: renumberedHex},
		// Written by the Rust encoder (gorse_common::encoding::gob::encode):
		// eleven entries in key order, one of each value type.
		{Name: "rust encoder: params", Target: "params", Hex: rustParams},
	}
	for i := range cases {
		decodeOne(&cases[i])
	}
	return cases
}

func decodeOne(c *DecodeCase) {
	data, err := hex.DecodeString(c.Hex)
	if err != nil {
		panic(c.Name + ": " + err.Error())
	}
	dec := gob.NewDecoder(bytes.NewBuffer(data))
	var value any
	switch c.Target {
	case "int64":
		var v int64
		err = dec.Decode(&v)
		value = strconv.FormatInt(v, 10)
	case "int32":
		var v int32
		err = dec.Decode(&v)
		value = strconv.FormatInt(int64(v), 10)
	case "float32":
		var v float32
		err = dec.Decode(&v)
		value = f32(v)
	case "float64":
		var v float64
		err = dec.Decode(&v)
		value = f64(v)
	case "bool":
		var v bool
		err = dec.Decode(&v)
		value = v
	case "string":
		var v string
		err = dec.Decode(&v)
		value = hex.EncodeToString([]byte(v))
	case "[]int":
		var v []int
		err = dec.Decode(&v)
		d := []string{}
		for _, x := range v {
			d = append(d, strconv.Itoa(x))
		}
		value = d
	case "params":
		var v model.Params
		err = dec.Decode(&v)
		value = describeParams(v)
	default:
		panic("unknown target " + c.Target)
	}
	if err != nil {
		c.Error = err.Error()
		return
	}
	c.OK, c.Value = true, value
}

func writeJSON(path string, v any) {
	data, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		panic(err)
	}
	if err = os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
		panic(err)
	}
}

func main() {
	out := flag.String("out", "", "output JSON file for encodings")
	decodeOut := flag.String("decode-out", "", "output JSON file for decodings")
	child := flag.Int("case", -1, "internal: encode one case in this process")
	childRenumbered := flag.Bool("renumbered", false, "internal: encode the renumbered case in this process")
	flag.Parse()
	if *child >= 0 {
		fmt.Print(run(*child))
		return
	}
	if *childRenumbered {
		fmt.Print(renumbered())
		return
	}
	self, err := os.Executable()
	if err != nil {
		panic(err)
	}
	var cases []Case
	for i, sp := range specs() {
		cmd := exec.Command(self, "-case", strconv.Itoa(i))
		cmd.Stderr = os.Stderr
		hexOut, err := cmd.Output()
		if err != nil {
			panic(err)
		}
		cases = append(cases, Case{Name: sp.name, Kind: sp.kind, Value: sp.describe, Hex: string(hexOut), Deterministic: sp.deterministic})
	}
	// Plain encodings (no gob).
	var b bytes.Buffer
	if err := encoding.WriteSlice(&b, []float32{1, 2, 3, 4}); err != nil {
		panic(err)
	}
	cases = append(cases, Case{Name: "slice float32", Kind: "slice-f32", Value: []string{f32(1), f32(2), f32(3), f32(4)}, Hex: hex.EncodeToString(b.Bytes()), Deterministic: true})
	b.Reset()
	if err := encoding.WriteString(&b, "abc"); err != nil {
		panic(err)
	}
	cases = append(cases, Case{Name: "string abc", Kind: "raw-string", Value: "abc", Hex: hex.EncodeToString(b.Bytes()), Deterministic: true})
	b.Reset()
	if err := encoding.WriteBytes(&b, []byte{0, 1, 0xff}); err != nil {
		panic(err)
	}
	cases = append(cases, Case{Name: "bytes 0001ff", Kind: "raw-bytes", Value: "0001ff", Hex: hex.EncodeToString(b.Bytes()), Deterministic: true})
	writeJSON(*out, cases)
	cmd := exec.Command(self, "-renumbered")
	cmd.Stderr = os.Stderr
	renumberedHex, err := cmd.Output()
	if err != nil {
		panic(err)
	}
	writeJSON(*decodeOut, decodeCases(string(renumberedHex)))
}
