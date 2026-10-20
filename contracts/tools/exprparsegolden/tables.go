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

// The test tables of expr-lang's parser, lexer and printer tests, read from
// their source, for the Rust port to run as its own tests
// (parser_go_tests.json). The tables are Go composite literals; tableValue
// turns them into JSON:
//
//   - &XNode{Field: v, ...} becomes {"node": "XNode", "fields": {...}},
//     with only the fields the literal sets (the rest are Go zero values);
//   - []byte("...") and []byte{...} become {"bytes": "<hex>"};
//   - a row of a table becomes an object keyed by the struct's field names;
//   - strings, numbers, booleans, nil and lists become themselves, and the
//     lexer's Kind constants (Number, EOF, ...) their names.
//
// Anything else panics, so a change in the tables' shape is noticed.

import (
	"encoding/hex"
	"fmt"
	goast "go/ast"
	goparser "go/parser"
	gotoken "go/token"
	"path/filepath"
	"strconv"
	"strings"
	"unicode/utf8"
)

// A table: the variable a test function of a file assigns it to.
type tableRef struct {
	key, file, function, variable string
}

var tableRefs = []tableRef{
	{"parse", "parser/parser_test.go", "TestParse", "tests"},
	{"parse_error", "parser/parser_test.go", "TestParse_error", "tests"},
	{"optional_chaining", "parser/parser_test.go", "TestParse_optional_chaining", "parseTests"},
	{"node_budget", "parser/parser_test.go", "TestNodeBudget", "tests"},
	{"lex", "parser/lexer/lexer_test.go", "TestLex", "tests"},
	{"print", "ast/print_test.go", "TestPrint", "tests"},
}

var lexerKinds = map[string]bool{
	"Identifier": true, "Number": true, "String": true, "Bytes": true,
	"Operator": true, "Bracket": true, "EOF": true,
}

// tableValue converts a Go expression of a test table; fields names the
// fields of the row struct when e is a row.
func tableValue(e goast.Expr, fields []string) any {
	if s, ok := constString(e); ok {
		if !utf8.ValidString(s) {
			// The Rust port takes text; the caller drops such rows.
			return invalidUTF8{}
		}
		return s
	}
	switch e := e.(type) {
	case *goast.BasicLit:
		switch e.Kind {
		case gotoken.INT:
			n, err := strconv.ParseInt(e.Value, 0, 64)
			if err != nil {
				panic(err)
			}
			return n
		case gotoken.FLOAT:
			f, err := strconv.ParseFloat(e.Value, 64)
			if err != nil {
				panic(err)
			}
			return f
		}
	case *goast.Ident:
		switch {
		case e.Name == "true":
			return true
		case e.Name == "false":
			return false
		case e.Name == "nil":
			return nil
		case lexerKinds[e.Name]:
			return e.Name
		}
	case *goast.UnaryExpr:
		if e.Op == gotoken.AND {
			return tableValue(e.X, nil)
		}
		if e.Op == gotoken.SUB {
			switch v := tableValue(e.X, nil).(type) {
			case int64:
				return -v
			case float64:
				return -v
			}
		}
	case *goast.CallExpr:
		// []byte("...")
		if t, ok := e.Fun.(*goast.ArrayType); ok && len(e.Args) == 1 {
			if elt, ok := t.Elt.(*goast.Ident); ok && elt.Name == "byte" {
				if s, ok := constString(e.Args[0]); ok {
					return map[string]any{"bytes": hex.EncodeToString([]byte(s))}
				}
			}
		}
	case *goast.CompositeLit:
		return compositeValue(e, fields)
	}
	panic(fmt.Sprintf("unsupported expression in a test table: %T %#v", e, e))
}

type invalidUTF8 struct{}

func compositeValue(e *goast.CompositeLit, fields []string) any {
	switch t := e.Type.(type) {
	case *goast.ArrayType:
		if elt, ok := t.Elt.(*goast.Ident); ok && elt.Name == "byte" {
			// []byte{255, 0}
			var b []byte
			for _, elt := range e.Elts {
				n, ok := tableValue(elt, nil).(int64)
				if !ok || n < 0 || n > 255 {
					panic("unsupported []byte element")
				}
				b = append(b, byte(n))
			}
			return map[string]any{"bytes": hex.EncodeToString(b)}
		}
		var rowFields []string
		if st, ok := t.Elt.(*goast.StructType); ok {
			for _, f := range st.Fields.List {
				for _, name := range f.Names {
					rowFields = append(rowFields, name.Name)
				}
			}
		}
		list := []any{}
		for _, elt := range e.Elts {
			list = append(list, tableValue(elt, rowFields))
		}
		return list
	case *goast.Ident, *goast.SelectorExpr:
		name := typeName(t)
		if strings.HasSuffix(name, "Node") {
			return map[string]any{"node": name, "fields": keyedFields(e)}
		}
		// file.Location and the like.
		return keyedFields(e)
	case nil:
		// A row, or an element of a []Token whose type is elided.
		if len(e.Elts) > 0 {
			if _, keyed := e.Elts[0].(*goast.KeyValueExpr); keyed {
				return keyedFields(e)
			}
		}
		if len(fields) != len(e.Elts) {
			panic(fmt.Sprintf("a row has %d values for fields %v", len(e.Elts), fields))
		}
		row := map[string]any{}
		for i, elt := range e.Elts {
			row[fields[i]] = tableValue(elt, nil)
		}
		return row
	}
	panic(fmt.Sprintf("unsupported composite literal type %T", e.Type))
}

func typeName(e goast.Expr) string {
	switch e := e.(type) {
	case *goast.Ident:
		return e.Name
	case *goast.SelectorExpr:
		return e.Sel.Name
	}
	panic(fmt.Sprintf("unsupported type %T", e))
}

func keyedFields(e *goast.CompositeLit) map[string]any {
	out := map[string]any{}
	for _, elt := range e.Elts {
		kv, ok := elt.(*goast.KeyValueExpr)
		if !ok {
			panic("mixed keyed and positional fields")
		}
		out[kv.Key.(*goast.Ident).Name] = tableValue(kv.Value, nil)
	}
	return out
}

// hasInvalid reports whether v holds a string that is not UTF-8.
func hasInvalid(v any) bool {
	switch v := v.(type) {
	case invalidUTF8:
		return true
	case []any:
		for _, x := range v {
			if hasInvalid(x) {
				return true
			}
		}
	case map[string]any:
		for _, x := range v {
			if hasInvalid(x) {
				return true
			}
		}
	}
	return false
}

// findTable returns the composite literal a function assigns to a variable.
func findTable(f *goast.File, function, variable string) *goast.CompositeLit {
	var found *goast.CompositeLit
	for _, decl := range f.Decls {
		fn, ok := decl.(*goast.FuncDecl)
		if !ok || fn.Name.Name != function {
			continue
		}
		goast.Inspect(fn.Body, func(n goast.Node) bool {
			switch n := n.(type) {
			case *goast.AssignStmt:
				for i, lhs := range n.Lhs {
					if id, ok := lhs.(*goast.Ident); ok && id.Name == variable && i < len(n.Rhs) {
						if lit, ok := n.Rhs[i].(*goast.CompositeLit); ok && found == nil {
							found = lit
						}
					}
				}
			case *goast.ValueSpec:
				for i, name := range n.Names {
					if name.Name == variable && i < len(n.Values) {
						if lit, ok := n.Values[i].(*goast.CompositeLit); ok && found == nil {
							found = lit
						}
					}
				}
			}
			return true
		})
	}
	if found == nil {
		panic(fmt.Sprintf("no table %s in %s", variable, function))
	}
	return found
}

// tokenValues turns the Value of each expected token of the lexer table
// into hex, as the golden files hold token values.
func tokenValues(rows []any) {
	for _, row := range rows {
		for _, token := range row.(map[string]any)["tokens"].([]any) {
			t := token.(map[string]any)
			v, _ := t["Value"].(string)
			t["Value"] = hex.EncodeToString([]byte(v))
		}
	}
}

// goTests reads the tables; rows holding a string that is not UTF-8 are
// counted per table under skipped_invalid_utf8.
func goTests(dir string) map[string]any {
	out := map[string]any{}
	skipped := map[string]int{}
	files := map[string]*goast.File{}
	for _, ref := range tableRefs {
		f, ok := files[ref.file]
		if !ok {
			var err error
			f, err = goparser.ParseFile(gotoken.NewFileSet(), filepath.Join(dir, ref.file), nil, 0)
			if err != nil {
				panic(err)
			}
			files[ref.file] = f
		}
		rows := []any{}
		for _, row := range tableValue(findTable(f, ref.function, ref.variable), nil).([]any) {
			if hasInvalid(row) {
				skipped[ref.key]++
				continue
			}
			rows = append(rows, row)
		}
		if ref.key == "lex" {
			tokenValues(rows)
		}
		out[ref.key] = rows
	}
	_, named := goStrings(filepath.Join(dir, "parser/lexer/lexer_test.go"))
	out["lex_error"] = named["errorTests"]
	out["skipped_invalid_utf8"] = skipped
	return out
}
