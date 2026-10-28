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

// Command datatypesgolden writes golden data produced by storage/data for the
// Rust port's tests (crates/gorse-storage/src/data/types.rs):
//
//	go run ./contracts/tools/datatypesgolden -out crates/gorse-storage/tests/data/types_golden.json
//
// It records two kinds of case:
//
//   - marshal: a JSON document given to json.Unmarshal into one of the record
//     types, and the json.Marshal of the result. This pins the JSON names,
//     the omitted UpdateAt, the nil and empty slices, the flattened embedded
//     structs, the time formats and the numbers.
//   - labels: a Go value of the kind labels hold, written as a tagged tree,
//     with the error ValidateLabels returns for it (or none). The tree keeps
//     the Go types apart: json.Number is "number", float64 is "float64".
//     Maps are given with at most one invalid entry, since Go's map iteration
//     order is random and the error would otherwise depend on it.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"

	"github.com/gorse-io/gorse/storage/data"
)

// MarshalCase is one round trip through a record type.
type MarshalCase struct {
	Type   string `json:"type"`
	Name   string `json:"name"`
	Input  string `json:"input"`
	Output string `json:"output"`
}

// LabelsCase is one ValidateLabels call and what Go returned.
type LabelsCase struct {
	Name   string  `json:"name"`
	Labels any     `json:"labels"`
	Error  *string `json:"error"`
}

// tree writes a Go label value as a tagged tree.
func tree(v any) any {
	switch x := v.(type) {
	case nil:
		return map[string]any{"kind": "nil"}
	case bool:
		return map[string]any{"kind": "bool", "value": x}
	case float64:
		return map[string]any{"kind": "float64", "value": x}
	case json.Number:
		return map[string]any{"kind": "number", "text": x.String()}
	case string:
		return map[string]any{"kind": "string", "value": x}
	case []any:
		items := make([]any, len(x))
		for i, e := range x {
			items[i] = tree(e)
		}
		return map[string]any{"kind": "array", "items": items}
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		entries := make([]any, 0, len(keys))
		for _, k := range keys {
			entries = append(entries, map[string]any{"key": k, "value": tree(x[k])})
		}
		return map[string]any{"kind": "object", "entries": entries}
	default:
		panic(fmt.Sprintf("no tree for %T", v))
	}
}

func marshalCase(typ, name, input string) MarshalCase {
	var v any
	switch typ {
	case "Item":
		v = &data.Item{}
	case "ScoredItem":
		v = &data.ScoredItem{}
	case "ItemPatch":
		v = &data.ItemPatch{}
	case "User":
		v = &data.User{}
	case "UserPatch":
		v = &data.UserPatch{}
	case "FeedbackKey":
		v = &data.FeedbackKey{}
	case "Feedback":
		v = &data.Feedback{}
	default:
		panic("unknown type " + typ)
	}
	if err := json.Unmarshal([]byte(input), v); err != nil {
		panic(fmt.Sprintf("%s %s: %v", typ, name, err))
	}
	out, err := json.Marshal(v)
	if err != nil {
		panic(fmt.Sprintf("%s %s: %v", typ, name, err))
	}
	return MarshalCase{Type: typ, Name: name, Input: input, Output: string(out)}
}

func labelsCase(name string, labels any) LabelsCase {
	c := LabelsCase{Name: name, Labels: tree(labels)}
	if err := data.ValidateLabels(labels); err != nil {
		text := err.Error()
		c.Error = &text
	}
	return c
}

func main() {
	out := flag.String("out", "types_golden.json", "where to write the golden JSON")
	flag.Parse()

	marshal := []MarshalCase{
		marshalCase("Item", "all_fields", `{"ItemId":"1","IsHidden":true,"Categories":["a","b"],"Timestamp":"2000-10-01T12:30:00.5+08:00","Labels":{"z":1.50,"a":[1e2,"x"]},"Comment":"<c>&"}`),
		marshalCase("Item", "missing_fields", `{"ItemId":"2"}`),
		marshalCase("Item", "null_categories", `{"ItemId":"3","Categories":null,"Labels":null}`),
		marshalCase("Item", "empty_categories", `{"ItemId":"4","Categories":[]}`),
		marshalCase("Item", "unknown_and_update_at_ignored", `{"UpdateAt":"2001-01-01T00:00:00Z","ItemId":"5","Extra":1}`),
		marshalCase("Item", "utc_time_with_nanoseconds", `{"ItemId":"6","Timestamp":"2020-01-02T03:04:05.123456789Z"}`),
		marshalCase("Item", "big_number_label", `{"ItemId":"7","Labels":12345678901234567890}`),
		marshalCase("Item", "exponent_label", `{"ItemId":"8","Labels":[1e21, 1e-7, 0.000001]}`),
		marshalCase("ScoredItem", "flattened_item", `{"ItemId":"9","Score":0.25,"Comment":"c","IsHidden":true}`),
		marshalCase("ItemPatch", "empty_patch", `{}`),
		marshalCase("ItemPatch", "set_some", `{"IsHidden":null,"Categories":[],"Timestamp":null,"Labels":null,"Comment":"x"}`),
		marshalCase("ItemPatch", "set_all", `{"IsHidden":false,"Categories":["q"],"Timestamp":"2010-05-06T07:08:09Z","Labels":{"k":[true]},"Comment":""}`),
		marshalCase("User", "labels_list", `{"UserId":"u","Labels":["x","y"],"Comment":""}`),
		marshalCase("User", "update_at_skipped", `{"UserId":"v","UpdateAt":"2001-01-01T00:00:00Z"}`),
		marshalCase("UserPatch", "bool_label", `{"Labels":{"b":true}}`),
		marshalCase("UserPatch", "empty_patch", `{}`),
		marshalCase("FeedbackKey", "key", `{"FeedbackType":"star","UserId":"u","ItemId":"i"}`),
		marshalCase("Feedback", "all_fields", `{"FeedbackType":"star","UserId":"u","ItemId":"i","Value":0.1,"Timestamp":"2020-01-01T00:00:00Z","Updated":"2020-01-02T03:04:05.5Z","Labels":[1,2],"Comment":"c"}`),
		marshalCase("Feedback", "value_exponent", `{"FeedbackType":"like","UserId":"u","ItemId":"i","Value":1e21,"Labels":"s"}`),
	}

	labels := []LabelsCase{
		labelsCase("nil", nil),
		labelsCase("empty_list", []any{}),
		labelsCase("strings", []any{"a", "b"}),
		labelsCase("numbers", []any{jsonNumber("1"), jsonNumber("2.50")}),
		labelsCase("mixed_string_first", []any{"a", jsonNumber("1")}),
		labelsCase("mixed_number_first", []any{jsonNumber("1"), "a"}),
		labelsCase("nil_first", []any{nil}),
		labelsCase("bool_first", []any{true}),
		labelsCase("nested_list_first", []any{[]any{jsonNumber("1")}}),
		labelsCase("object_first", []any{map[string]any{"a": jsonNumber("1")}}),
		labelsCase("float_first", []any{1.5, "x"}),
		labelsCase("float_big", []any{1e21}),
		labelsCase("html_in_string", []any{"<a>", jsonNumber("1")}),
		labelsCase("string_then_nil", []any{"x", "y", nil}),
		labelsCase("map_of_valid", map[string]any{"city": "wenzhou", "tags": []any{"1", "2"}, "n": []any{jsonNumber("1")}}),
		labelsCase("nested_maps", map[string]any{"x": map[string]any{"y": map[string]any{"z": []any{jsonNumber("1"), jsonNumber("2")}}}}),
		labelsCase("map_with_bool", map[string]any{"ok": true}),
		labelsCase("map_with_float", map[string]any{"price": 1.5}),
		labelsCase("map_list_of_float", map[string]any{"a": []any{1.5}}),
		labelsCase("map_list_mixed", map[string]any{"a": []any{jsonNumber("1"), "2"}}),
		labelsCase("map_deep_bool", map[string]any{"a": map[string]any{"b": []any{true}}}),
		labelsCase("string_value", "label"),
		labelsCase("number_value", jsonNumber("1")),
		labelsCase("exponent_number", jsonNumber("1e2")),
		labelsCase("bool_value", true),
		labelsCase("float_value", 1.5),
	}

	writeJSON(*out, map[string]any{
		"marshal": marshal,
		"labels":  labels,
	})
	fmt.Printf("wrote %d marshal and %d labels cases to %s\n", len(marshal), len(labels), *out)
}

// jsonNumber is a json.Number, as a decoder with UseNumber gives numbers.
func jsonNumber(text string) json.Number {
	return json.Number(text)
}

// writeJSON writes v as indented JSON to path.
func writeJSON(path string, v any) {
	data, err := json.MarshalIndent(v, "", " ")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
