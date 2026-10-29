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

// Command rendergolden renders reranker document templates the way
// logics' rankerTemplates does: gonja parses the template, and an item is
// executed under the name "item". The Rust port renders the same cases with
// minijinja (crates/gorse-logics/src/reranker.rs), and its test compares the
// two outputs.
//
// It reads a JSON array of cases on stdin. Each case has a template and the
// labels of an item. The labels are decoded with encoding/json, as the data
// store decodes them into `any`, so every number is a float64. It writes one
// result per case, in the same order: the rendered document, or the error.
// gonja can panic while it renders a value; that panic is the case's error,
// as Go's reranker would see it when the panic is recovered.
//
//	go run ./contracts/tools/rendergolden < cases.json
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/gorse-io/gorse/storage/data"
	"github.com/nikolalohinski/gonja/v2"
	"github.com/nikolalohinski/gonja/v2/exec"
)

// testCase is one template and the labels of the item it renders.
type testCase struct {
	Template string `json:"template"`
	Labels   any    `json:"labels"`
}

// result is one rendered case: the document, or the error it gave.
type result struct {
	Document string `json:"document"`
	Error    string `json:"error,omitempty"`
}

func main() {
	if err := run(os.Stdin, os.Stdout); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run(in io.Reader, out io.Writer) error {
	input, err := io.ReadAll(in)
	if err != nil {
		return err
	}
	var cases []testCase
	if err := json.Unmarshal(input, &cases); err != nil {
		return err
	}
	results := make([]result, len(cases))
	for i, testCase := range cases {
		document, err := render(testCase)
		results[i] = result{Document: document}
		if err != nil {
			results[i].Error = err.Error()
		}
	}
	return json.NewEncoder(out).Encode(results)
}

// render executes a case's template as logics' `render` does, and turns a
// panic into an error.
func render(testCase testCase) (document string, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("panic: %v", r)
		}
	}()
	tpl, err := gonja.FromString(testCase.Template)
	if err != nil {
		return "", err
	}
	item := &data.Item{ItemId: "item", Labels: testCase.Labels}
	var builder strings.Builder
	if err := tpl.Execute(&builder, exec.NewContext(map[string]any{"item": item})); err != nil {
		return "", err
	}
	return builder.String(), nil
}
