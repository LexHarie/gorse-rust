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

// Command metagolden records what Gorse's meta store (storage/meta at the
// pinned commit) does, as golden data for the Rust port in
// crates/gorse-storage/src/meta (WP-2-11):
//
//   - the text the SQLite store writes into update_time for a node, the text
//     the column holds, and the time the driver reads back from it
//     (times);
//   - Model.ToJSON for a set of models (to_json);
//   - Model.FromJSON for a set of inputs, with each parameter's dynamic type
//     and its JSON value (from_json);
//   - Model.FromJSON followed by Model.ToJSON for inputs whose parameters are
//     arrays and objects, the output being byte for byte what Go writes
//     (round_trip).
//
// Usage:
//
//	go run ./contracts/tools/metagolden \
//		-out crates/gorse-storage/tests/data/meta_golden.json
package main

import (
	"database/sql"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"time"

	"github.com/gorse-io/gorse/model"
	"github.com/gorse-io/gorse/storage/meta"
	_ "modernc.org/sqlite"
)

type golden struct {
	GoVersion string          `json:"go_version"`
	Times     []timeCase      `json:"times"`
	ToJSON    []jsonCase      `json:"to_json"`
	FromJSON  []fromCase      `json:"from_json"`
	RoundTrip []roundTripCase `json:"round_trip"`
}

type timeCase struct {
	Name string `json:"name"`
	// Year, month, day, hour, minute, second and nanosecond, in UTC.
	Date [7]int `json:"date"`
	// Time.String() of the UTC time, which the store writes.
	Written string `json:"written"`
	// The text the update_time column holds after UpdateNode.
	Stored string `json:"stored"`
	// Time.String() of the time the driver reads from the column.
	Read string `json:"read"`
}

type jsonCase struct {
	Name string `json:"name"`
	// ToJSON's output.
	JSON string `json:"json"`
}

type roundTripCase struct {
	Name string `json:"name"`
	// The JSON given to FromJSON.
	Input string `json:"input"`
	// ToJSON's output after FromJSON.
	JSON string `json:"json"`
}

type paramResult struct {
	Name string `json:"name"`
	// The dynamic Go type, as %T prints it.
	GoType string `json:"go_type"`
	// The value as json.Marshal writes it.
	Value string `json:"value"`
}

type fromCase struct {
	Name string `json:"name"`
	// The input given to FromJSON.
	Input string `json:"input"`
	// The model's fields after FromJSON, for a Model[int].
	ID        int64         `json:"id"`
	Type      string        `json:"type"`
	ParamsNil bool          `json:"params_nil"`
	Params    []paramResult `json:"params"`
	Score     int           `json:"score"`
	// Whether FromJSON returned an error.
	Err bool `json:"err"`
}

func main() {
	out := flag.String("out", "", "file to write the golden JSON to")
	flag.Parse()
	if *out == "" {
		log.Fatal("-out is required")
	}
	g := golden{GoVersion: runtime.Version()}
	var err error
	if g.Times, err = timeCases(); err != nil {
		log.Fatal(err)
	}
	g.ToJSON = toJSONCases()
	g.FromJSON = fromJSONCases()
	g.RoundTrip = roundTripCases()
	data, err := json.MarshalIndent(g, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}

// timeCases stores each time with the meta store, reads the column back two
// ways, and records the three texts.
func timeCases() ([]timeCase, error) {
	utc := time.UTC
	cases := []struct {
		name string
		t    time.Time
	}{
		{"fraction", time.Date(2026, 10, 28, 8, 10, 4, 123456789, utc)},
		{"whole_seconds", time.Date(1996, 4, 8, 0, 0, 0, 0, utc)},
		{"half_second", time.Date(2023, 1, 1, 0, 0, 0, 500000000, utc)},
		{"micro", time.Date(2026, 1, 2, 3, 4, 5, 678901000, utc)},
		{"local_zone", time.Date(2026, 10, 28, 16, 0, 0, 0, time.FixedZone("CST", 8*3600))},
	}
	var out []timeCase
	for _, c := range cases {
		stored, read, err := storeAndRead(c.t)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", c.name, err)
		}
		u := c.t.UTC()
		out = append(out, timeCase{
			Name:    c.name,
			Date:    [7]int{u.Year(), int(u.Month()), u.Day(), u.Hour(), u.Minute(), u.Second(), u.Nanosecond()},
			Written: u.String(),
			Stored:  stored,
			Read:    read,
		})
	}
	return out, nil
}

// storeAndRead writes one node through the meta store and returns the column
// text and the time the driver reads from it.
func storeAndRead(t time.Time) (string, string, error) {
	dir, err := os.MkdirTemp("", "metagolden")
	if err != nil {
		return "", "", err
	}
	defer os.RemoveAll(dir)
	file := filepath.Join(dir, "meta.sqlite3")
	db, err := meta.Open("sqlite://"+file, time.Hour)
	if err != nil {
		return "", "", err
	}
	if err := db.Init(); err != nil {
		return "", "", err
	}
	if err := db.UpdateNode(&meta.Node{
		UUID:       "node",
		Hostname:   "localhost",
		Type:       "master",
		Version:    "v0.1.0",
		UpdateTime: t,
	}); err != nil {
		return "", "", err
	}
	if err := db.Close(); err != nil {
		return "", "", err
	}
	raw, err := sql.Open("sqlite", file)
	if err != nil {
		return "", "", err
	}
	defer raw.Close()
	var stored string
	if err := raw.QueryRow("SELECT CAST(update_time AS TEXT) FROM nodes").Scan(&stored); err != nil {
		return "", "", err
	}
	var read time.Time
	if err := raw.QueryRow("SELECT update_time FROM nodes").Scan(&read); err != nil {
		return "", "", err
	}
	return stored, read.String(), nil
}

// toJSONCases are the models of the to_json cases. Rust builds the same
// models under the same names.
func toJSONCases() []jsonCase {
	models := []struct {
		name string
		json func() string
	}{
		{"int_score", func() string {
			m := meta.Model[int]{ID: 7, Type: "als", Score: 42}
			return m.ToJSON()
		}},
		{"params", func() string {
			m := meta.Model[int]{
				ID:   7,
				Type: "als",
				Params: model.Params{
					"Lr":           0.007,
					"NFactors":     80,
					"HiddenLayers": []int{64, 32},
					"Optimizer":    "adam",
					"AutoScale":    true,
					"Missing":      nil,
					"Alpha":        float32(0.007),
					"BatchSize":    int64(256),
					"InitStdDev":   float32(0.1),
					"RandomState":  int32(-3),
				},
				Score: 42,
			}
			return m.ToJSON()
		}},
		{"number_forms", func() string {
			m := meta.Model[float64]{
				ID:   1,
				Type: "forms",
				Params: model.Params{
					"big":     1e21,
					"small":   1e-7,
					"plain":   123456789.0,
					"sum":     0.1 + 0.2,
					"negzero": negativeZero(),
					"tiny32":  float32(1e-7),
					"big32":   float32(1e21),
				},
				Score: 0.5,
			}
			return m.ToJSON()
		}},
		{"string_escapes", func() string {
			m := meta.Model[int]{
				ID:   2,
				Type: "<a&b>  \"\\\n\t\u0001é",
				Params: model.Params{
					"s": "<a&b>  \"\\\n\t\u0001é\b\f\r",
				},
			}
			return m.ToJSON()
		}},
		{"empty_params", func() string {
			m := meta.Model[int]{ID: 3, Type: "none", Params: model.Params{}}
			return m.ToJSON()
		}},
		{"nil_params", func() string {
			m := meta.Model[int]{ID: 4, Type: "nil"}
			return m.ToJSON()
		}},
	}
	var out []jsonCase
	for _, m := range models {
		out = append(out, jsonCase{Name: m.name, JSON: m.json()})
	}
	return out
}

// negativeZero is -0.0 as a float64. A Go constant -0.0 is 0.0, so the sign comes
// from a variable.
func negativeZero() float64 {
	zero := 0.0
	return -zero
}

// fromJSONCases are the inputs of the from_json cases. Each one starts from
// the model its name describes; Rust starts from the same model.
func fromJSONCases() []fromCase {
	cases := []struct {
		name  string
		input string
		start func() meta.Model[int]
	}{
		{"fresh_all_fields", `{"ID":5,"Type":"als","Params":{"Lr":0.5,"NFactors":80,"Optimizer":"adam","Gone":null,"On":true,"HiddenLayers":[64,32],"Nested":{"x":1}},"Score":9}`, fresh},
		{"absent_fields_keep_values", `{"Params":{"Lr":0.5}}`, withParams},
		{"null_params_clears", `{"Params":null}`, withParams},
		{"null_fields_are_noops", `{"ID":null,"Type":null,"Score":null}`, withParams},
		{"top_level_null_is_noop", `null`, withParams},
		{"numbers_are_float64", `{"Params":{"NFactors":80}}`, fresh},
		{"array_param", `{"Params":{"HiddenLayers":[64,32]}}`, fresh},
		{"object_param", `{"Params":{"o":{"x":1}}}`, fresh},
		{"params_not_an_object", `{"Params":[1]}`, fresh},
		{"invalid_json", `{"ID":`, fresh},
	}
	var out []fromCase
	for _, c := range cases {
		m := c.start()
		err := m.FromJSON(c.input)
		fc := fromCase{
			Name:      c.name,
			Input:     c.input,
			ID:        m.ID,
			Type:      m.Type,
			ParamsNil: m.Params == nil,
			Score:     m.Score,
			Err:       err != nil,
		}
		fc.Params = paramsOf(m.Params)
		out = append(out, fc)
	}
	return out
}

// fresh is the zero model.
func fresh() meta.Model[int] {
	return meta.Model[int]{}
}

// withParams is a model that already has parameters, one of them an int, so
// that FromJSON's merge into an existing map shows.
func withParams() meta.Model[int] {
	return meta.Model[int]{
		ID:     5,
		Type:   "old",
		Params: model.Params{"keep": 1, "Lr": 0.1},
		Score:  9,
	}
}

// roundTripCases decode each input into a zero model and encode it again.
// The inputs hold arrays and objects, whose numbers Go decodes as float64s
// and whose object keys Go sorts when it encodes them. Rust builds the same
// inputs under the same names.
func roundTripCases() []roundTripCase {
	cases := []struct {
		name  string
		input string
	}{
		{"nested_objects_and_arrays", `{"ID":1,"Type":"t","Params":{"Opt":{"b":1,"a":[true,null,"x<y&z>"]},"HiddenLayers":[64,32]},"Score":3}`},
		{"numbers", `{"Params":{"F":[1.50,1e2,0.1,1e21,-0,123456789012345678,1e-7,5e-324,-2.5e-5]}}`},
		{"empty_array_and_object", `{"Params":{"E":[],"O":{}}}`},
		{"duplicate_keys_keep_the_last", `{"Params":{"O":{"a":1,"a":2,"b":3}}}`},
		{"keys_sorted_by_bytes", `{"Params":{"b":[1],"a":[2],"B":[3],"é":[4],"Z":{"y":1,"x":2}}}`},
		{"string_escapes", `{"Params":{"S":["\u0001\b\f\n\r\t\"\\/\u2028\u2029é😀<>&"]}}`},
		{"nested_arrays_and_null", `{"Params":{"L":[{"k":[{}]},[[]],null,false]}}`},
		{"mixed_scalars_and_arrays", `{"Params":{"Lr":0.5,"HiddenLayers":[64,32],"Optimizer":"adam","On":true,"Gone":null}}`},
	}
	var out []roundTripCase
	for _, c := range cases {
		var m meta.Model[int]
		if err := m.FromJSON(c.input); err != nil {
			log.Fatalf("%s: %v", c.name, err)
		}
		out = append(out, roundTripCase{Name: c.name, Input: c.input, JSON: m.ToJSON()})
	}
	return out
}

// paramsOf lists the parameters sorted by name, with their dynamic types.
func paramsOf(params model.Params) []paramResult {
	names := make([]string, 0, len(params))
	for name := range params {
		names = append(names, string(name))
	}
	sort.Strings(names)
	var out []paramResult
	for _, name := range names {
		value := params[model.ParamName(name)]
		data, err := json.Marshal(value)
		if err != nil {
			log.Fatal(err)
		}
		out = append(out, paramResult{
			Name:   name,
			GoType: fmt.Sprintf("%T", value),
			Value:  string(data),
		})
	}
	return out
}
