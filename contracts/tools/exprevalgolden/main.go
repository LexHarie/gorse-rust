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

// Command exprevalgolden writes what expr-lang v1.17.8 does with a corpus of
// expressions, for the Rust port's checker, compiler and VM (gorse-expr):
//
//   - compile: expr.Compile of the source with the environment the corpus
//     names, as Gorse builds it in logics/non_personalized.go, item_to_item.go
//     and user_to_user.go. The verdict is the error text, or the root node's
//     reflect type and kind.
//   - run: expr.Run of the compiled program with the fixtures of that
//     environment. The result is the Go type and a tagged value, or the error
//     text.
//
// An expression that calls now() and yields a time or a duration is volatile:
// its result follows the clock, so only its compile verdict is compared.
//
// The corpus is a JSON array of {source, env, origin}; the fixtures are a JSON
// object of an item, a feedback list and a user, decoded with encoding/json
// into Gorse's storage/data types. Floats are written as their IEEE bits in
// hex, so both sides read them exactly.
package main

import (
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/expr-lang/expr"
	"github.com/expr-lang/expr/vm"
	"github.com/gorse-io/gorse/storage/data"
)

// Case is one entry of the corpus.
type Case struct {
	Source string `json:"source"`
	Env    string `json:"env"`
	Origin string `json:"origin"`
}

// Fixtures are the values the environments are run with.
type Fixtures struct {
	Item     data.Item       `json:"item"`
	Feedback []data.Feedback `json:"feedback"`
	User     data.User       `json:"user"`
}

// Verdict is what Compile says about a source.
type Verdict struct {
	OK    bool   `json:"ok"`
	Error string `json:"error,omitempty"`
	Type  string `json:"type,omitempty"`
	Kind  string `json:"kind,omitempty"`
}

// Outcome is what Run says about a compiled program.
type Outcome struct {
	Error  string `json:"error,omitempty"`
	Type   string `json:"type,omitempty"`
	Result any    `json:"result,omitempty"`
}

// Result is one line of the golden file.
type Result struct {
	Source   string   `json:"source"`
	Env      string   `json:"env"`
	Origin   string   `json:"origin"`
	Volatile bool     `json:"volatile"`
	Compile  Verdict  `json:"compile"`
	Run      *Outcome `json:"run,omitempty"`
}

func main() {
	corpusPath := flag.String("corpus", "", "the corpus JSON file")
	fixturesPath := flag.String("fixtures", "", "the fixtures JSON file")
	outPath := flag.String("out", "", "where to write the golden JSON file")
	flag.Parse()

	var cases []Case
	readJSON(*corpusPath, &cases)
	var fixtures Fixtures
	readJSON(*fixturesPath, &fixtures)

	results := make([]Result, 0, len(cases))
	for _, c := range cases {
		results = append(results, evaluate(c, fixtures))
	}
	out, err := json.MarshalIndent(results, "", "  ")
	if err != nil {
		fail(err)
	}
	if err := os.WriteFile(*outPath, append(out, '\n'), 0o644); err != nil {
		fail(err)
	}
}

// evaluate compiles one case and, when it compiles, runs it.
func evaluate(c Case, f Fixtures) Result {
	result := Result{Source: c.Source, Env: c.Env, Origin: c.Origin}
	program, err := compile(c.Source, c.Env)
	if err != nil {
		result.Compile = Verdict{OK: false, Error: err.Error()}
		return result
	}
	result.Compile = Verdict{
		OK:   true,
		Type: program.Node().Type().String(),
		Kind: program.Node().Type().Kind().String(),
	}
	outcome := &Outcome{}
	value, err := expr.Run(program, environment(c.Env, f))
	if err != nil {
		outcome.Error = err.Error()
	} else {
		outcome.Type = fmt.Sprintf("%T", value)
		outcome.Result = encode(value)
	}
	if strings.Contains(c.Source, "now()") {
		switch value.(type) {
		case time.Time, time.Duration:
			result.Volatile = true
			return result
		}
	}
	result.Run = outcome
	return result
}

// compile builds the program with the environment of the named Gorse site.
func compile(source, env string) (*vm.Program, error) {
	switch env {
	case "non_personalized":
		return expr.Compile(source, expr.Env(map[string]any{
			"item":     data.Item{},
			"feedback": []data.Feedback{},
		}))
	case "item":
		return expr.Compile(source, expr.Env(map[string]any{"item": data.Item{}}))
	case "user":
		return expr.Compile(source, expr.Env(map[string]any{"user": data.User{}}))
	case "none":
		return expr.Compile(source)
	}
	fail(fmt.Errorf("unknown environment %q", env))
	return nil, nil
}

// environment is the value map the named site passes to Run.
func environment(env string, f Fixtures) map[string]any {
	switch env {
	case "non_personalized":
		return map[string]any{"item": f.Item, "feedback": f.Feedback}
	case "item":
		return map[string]any{"item": f.Item}
	case "user":
		return map[string]any{"user": f.User}
	}
	return map[string]any{}
}

// encode turns a result into JSON with its Go type made explicit: numbers
// are tagged with their kind, bytes and invalid UTF-8 strings are hex, times
// and durations are tagged, and everything else Gorse can see is a list or a
// map.
func encode(v any) any {
	switch t := v.(type) {
	case nil:
		return nil
	case bool:
		return t
	case string:
		if utf8.ValidString(t) {
			return t
		}
		return map[string]any{"bytes_hex": hex.EncodeToString([]byte(t))}
	case []byte:
		return map[string]any{"bytes_hex": hex.EncodeToString(t)}
	case int:
		return map[string]any{"int": strconv.FormatInt(int64(t), 10)}
	case int8:
		return map[string]any{"int": strconv.FormatInt(int64(t), 10)}
	case int16:
		return map[string]any{"int": strconv.FormatInt(int64(t), 10)}
	case int32:
		return map[string]any{"int": strconv.FormatInt(int64(t), 10)}
	case int64:
		return map[string]any{"int": strconv.FormatInt(t, 10)}
	case uint:
		return map[string]any{"uint": strconv.FormatUint(uint64(t), 10)}
	case uint8:
		return map[string]any{"uint": strconv.FormatUint(uint64(t), 10)}
	case uint16:
		return map[string]any{"uint": strconv.FormatUint(uint64(t), 10)}
	case uint32:
		return map[string]any{"uint": strconv.FormatUint(uint64(t), 10)}
	case uint64:
		return map[string]any{"uint": strconv.FormatUint(t, 10)}
	case float64:
		return map[string]any{"float": fmt.Sprintf("%016x", math.Float64bits(t))}
	case float32:
		return map[string]any{"float": fmt.Sprintf("%016x", math.Float64bits(float64(t)))}
	case time.Time:
		return map[string]any{"time": t.Format(time.RFC3339Nano)}
	case time.Duration:
		return map[string]any{"duration_ns": strconv.FormatInt(int64(t), 10)}
	case map[string]any:
		out := make(map[string]any, len(t))
		for k, value := range t {
			out[k] = encode(value)
		}
		return map[string]any{"map": out}
	}
	rv := reflect.ValueOf(v)
	switch rv.Kind() {
	case reflect.Slice, reflect.Array:
		out := make([]any, rv.Len())
		for i := range out {
			out[i] = encode(rv.Index(i).Interface())
		}
		return map[string]any{"list": out}
	case reflect.Map:
		if rv.Type().Key().Kind() == reflect.String {
			keys := rv.MapKeys()
			names := make([]string, 0, len(keys))
			for _, key := range keys {
				names = append(names, key.String())
			}
			sort.Strings(names)
			out := make(map[string]any, len(names))
			for _, name := range names {
				out[name] = encode(rv.MapIndex(reflect.ValueOf(name)).Interface())
			}
			return map[string]any{"map": out}
		}
	}
	return map[string]any{"go": fmt.Sprintf("%T", v)}
}

func readJSON(path string, v any) {
	raw, err := os.ReadFile(path)
	if err != nil {
		fail(err)
	}
	if err := json.Unmarshal(raw, v); err != nil {
		fail(err)
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}
