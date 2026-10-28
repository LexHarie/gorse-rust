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

// The inputs and tables of yaml.v3's tests, read from its source in the
// module cache:
//
//   - every string the test files spell (a constant string expression,
//     possibly built with strings.Repeat or converted to []byte) joins the
//     corpus: the YAML documents of the test tables, and every other string
//     too, which are inputs like any other;
//   - limit_test.go's inputs are up to a megabyte, so they are kept as the
//     parts they are built from (Part);
//   - the rows of TestUnmarshal whose target is map[string]interface{} (or
//     M) keep the value the test expects, evaluated from its Go literal;
//   - the rows of TestUnmarshalErrors keep their error pattern.
//
// An expression the evaluators do not know panics, so a change in the
// tables' shape is noticed.

import (
	"fmt"
	goast "go/ast"
	goparser "go/parser"
	gotoken "go/token"
	"math"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

const yamlModule = "go.yaml.in/yaml/v3"

// moduleDir finds the yaml.v3 module source the build list pins.
func moduleDir() (version, dir string) {
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Version}} {{.Dir}}", yamlModule).Output()
	if err != nil {
		panic(err)
	}
	fields := strings.Fields(strings.TrimSpace(string(out)))
	if len(fields) != 2 || fields[0] != "v3.0.4" {
		panic(fmt.Sprintf("unexpected %s module: %q", yamlModule, out))
	}
	return fields[0], fields[1]
}

type namedInput struct {
	name, text string
}

type errorRow struct {
	index          int
	input, pattern string
}

type tables struct {
	inputs           []namedInput
	limits           []GoLimit
	unmarshal        []GoUnmarshal
	unmarshalErrors  []errorRow
	mergeTests       string
	mergeTestsNested string
	fuzzCrashers     []string
	issue117         string
}

func parseFile(dir, name string) (*gotoken.FileSet, *goast.File) {
	fset := gotoken.NewFileSet()
	f, err := goparser.ParseFile(fset, filepath.Join(dir, name), nil, 0)
	if err != nil {
		panic(err)
	}
	return fset, f
}

func readTables(dir string) *tables {
	t := &tables{}
	for _, name := range []string{"decode_test.go", "node_test.go", "encode_test.go"} {
		fset, f := parseFile(dir, name)
		for _, decl := range f.Decls {
			if g, ok := decl.(*goast.GenDecl); ok && g.Tok == gotoken.IMPORT {
				continue
			}
			goast.Inspect(decl, func(n goast.Node) bool {
				e, ok := n.(goast.Expr)
				if !ok {
					return true
				}
				s, ok := strValue(e)
				if !ok {
					return true
				}
				line := fset.Position(e.Pos()).Line
				t.inputs = append(t.inputs, namedInput{fmt.Sprintf("%s:%d", name, line), s})
				return false
			})
		}
	}

	_, decodeTest := parseFile(dir, "decode_test.go")
	for i, row := range tableRows(decodeTest, "unmarshalTests") {
		elts := row.Elts
		lit, ok := elts[1].(*goast.CompositeLit)
		if !ok || !isStringMapType(lit.Type) {
			continue
		}
		input, ok := strValue(elts[0])
		if !ok {
			panic("unmarshalTests: input")
		}
		t.unmarshal = append(t.unmarshal, GoUnmarshal{Index: i, Input: input, Want: encode(goValue(lit))})
	}
	for i, row := range tableRows(decodeTest, "unmarshalErrorTests") {
		input, ok1 := strValue(row.Elts[0])
		pattern, ok2 := strValue(row.Elts[1])
		if !ok1 || !ok2 {
			panic("unmarshalErrorTests: row")
		}
		t.unmarshalErrors = append(t.unmarshalErrors, errorRow{i, input, pattern})
	}
	t.mergeTests = packageString(decodeTest, "mergeTests")
	t.mergeTestsNested = packageString(decodeTest, "mergeTestsNested")
	for _, e := range funcComposite(decodeTest, "TestFuzzCrashers", "cases").Elts {
		s, ok := strValue(e)
		if !ok {
			panic("TestFuzzCrashers: case")
		}
		t.fuzzCrashers = append(t.fuzzCrashers, s)
	}
	t.issue117 = funcString(decodeTest, "TestIssue117", "data")

	_, limitTest := parseFile(dir, "limit_test.go")
	for _, row := range tableRows(limitTest, "limitTests") {
		var l GoLimit
		for _, elt := range row.Elts {
			kv := elt.(*goast.KeyValueExpr)
			switch kv.Key.(*goast.Ident).Name {
			case "name":
				l.Name, _ = strValue(kv.Value)
			case "data":
				l.Gen = partsValue(kv.Value)
			case "error":
				l.Error, _ = strValue(kv.Value)
			}
		}
		t.limits = append(t.limits, l)
	}
	return t
}

// tableRows returns the rows of a package-level table.
func tableRows(f *goast.File, variable string) []*goast.CompositeLit {
	for _, decl := range f.Decls {
		g, ok := decl.(*goast.GenDecl)
		if !ok {
			continue
		}
		for _, spec := range g.Specs {
			vs, ok := spec.(*goast.ValueSpec)
			if !ok {
				continue
			}
			for i, name := range vs.Names {
				if name.Name != variable {
					continue
				}
				var rows []*goast.CompositeLit
				for _, e := range vs.Values[i].(*goast.CompositeLit).Elts {
					rows = append(rows, e.(*goast.CompositeLit))
				}
				return rows
			}
		}
	}
	panic("no table " + variable)
}

// packageString returns a package-level string variable.
func packageString(f *goast.File, variable string) string {
	for _, decl := range f.Decls {
		g, ok := decl.(*goast.GenDecl)
		if !ok {
			continue
		}
		for _, spec := range g.Specs {
			vs, ok := spec.(*goast.ValueSpec)
			if !ok {
				continue
			}
			for i, name := range vs.Names {
				if name.Name == variable {
					s, ok := strValue(vs.Values[i])
					if !ok {
						panic("not a string: " + variable)
					}
					return s
				}
			}
		}
	}
	panic("no variable " + variable)
}

// funcAssigned returns the expression a function assigns to a variable.
func funcAssigned(f *goast.File, function, variable string) goast.Expr {
	var found goast.Expr
	for _, decl := range f.Decls {
		fn, ok := decl.(*goast.FuncDecl)
		if !ok || fn.Name.Name != function {
			continue
		}
		goast.Inspect(fn.Body, func(n goast.Node) bool {
			if a, ok := n.(*goast.AssignStmt); ok {
				for i, lhs := range a.Lhs {
					if id, ok := lhs.(*goast.Ident); ok && id.Name == variable && found == nil {
						found = a.Rhs[i]
					}
				}
			}
			return true
		})
	}
	if found == nil {
		panic(fmt.Sprintf("no %s in %s", variable, function))
	}
	return found
}

func funcComposite(f *goast.File, function, variable string) *goast.CompositeLit {
	return funcAssigned(f, function, variable).(*goast.CompositeLit)
}

func funcString(f *goast.File, function, variable string) string {
	s, ok := strValue(funcAssigned(f, function, variable))
	if !ok {
		panic(fmt.Sprintf("%s in %s is not a string", variable, function))
	}
	return s
}

func isSelector(e goast.Expr, pkg, name string) bool {
	s, ok := e.(*goast.SelectorExpr)
	if !ok {
		return false
	}
	id, ok := s.X.(*goast.Ident)
	return ok && id.Name == pkg && s.Sel.Name == name
}

// isByteConversion reports whether call is []byte(x).
func isByteConversion(call *goast.CallExpr) bool {
	t, ok := call.Fun.(*goast.ArrayType)
	if !ok || t.Len != nil || len(call.Args) != 1 {
		return false
	}
	id, ok := t.Elt.(*goast.Ident)
	return ok && id.Name == "byte"
}

// strValue evaluates a constant string expression: literals, +,
// strings.Repeat with constant arguments and []byte conversions.
func strValue(e goast.Expr) (string, bool) {
	switch e := e.(type) {
	case *goast.BasicLit:
		if e.Kind != gotoken.STRING {
			return "", false
		}
		s, err := strconv.Unquote(e.Value)
		if err != nil {
			panic(err)
		}
		return s, true
	case *goast.ParenExpr:
		return strValue(e.X)
	case *goast.BinaryExpr:
		if e.Op != gotoken.ADD {
			return "", false
		}
		a, ok1 := strValue(e.X)
		b, ok2 := strValue(e.Y)
		return a + b, ok1 && ok2
	case *goast.CallExpr:
		if isByteConversion(e) {
			return strValue(e.Args[0])
		}
		if isSelector(e.Fun, "strings", "Repeat") {
			s, ok1 := strValue(e.Args[0])
			n, ok2 := intValue(e.Args[1])
			return strings.Repeat(s, n), ok1 && ok2
		}
	}
	return "", false
}

// partsValue evaluates a constant string expression as parts.
func partsValue(e goast.Expr) []Part {
	switch e := e.(type) {
	case *goast.BasicLit, *goast.ParenExpr:
		s, ok := strValue(e)
		if !ok {
			panic("partsValue: not a string")
		}
		return []Part{{s, 1}}
	case *goast.BinaryExpr:
		if e.Op == gotoken.ADD {
			return append(partsValue(e.X), partsValue(e.Y)...)
		}
	case *goast.CallExpr:
		if isByteConversion(e) {
			return partsValue(e.Args[0])
		}
		if isSelector(e.Fun, "strings", "Repeat") {
			s, ok1 := strValue(e.Args[0])
			n, ok2 := intValue(e.Args[1])
			if !ok1 || !ok2 {
				panic("partsValue: strings.Repeat")
			}
			return []Part{{s, n}}
		}
	}
	panic(fmt.Sprintf("partsValue: unsupported %T", e))
}

// intValue evaluates a constant integer expression.
func intValue(e goast.Expr) (int, bool) {
	switch e := e.(type) {
	case *goast.BasicLit:
		if e.Kind != gotoken.INT {
			return 0, false
		}
		n, err := strconv.ParseInt(e.Value, 0, 64)
		if err != nil {
			panic(err)
		}
		return int(n), true
	case *goast.ParenExpr:
		return intValue(e.X)
	case *goast.BinaryExpr:
		a, ok1 := intValue(e.X)
		b, ok2 := intValue(e.Y)
		if !ok1 || !ok2 {
			return 0, false
		}
		switch e.Op {
		case gotoken.ADD:
			return a + b, true
		case gotoken.SUB:
			return a - b, true
		case gotoken.MUL:
			return a * b, true
		case gotoken.QUO:
			return a / b, true
		}
	}
	return 0, false
}

// isStringMapType reports whether a composite literal's type is
// map[string]interface{} or the tests' M.
func isStringMapType(e goast.Expr) bool {
	switch e := e.(type) {
	case *goast.Ident:
		return e.Name == "M"
	case *goast.MapType:
		k, ok := e.Key.(*goast.Ident)
		_, iface := e.Value.(*goast.InterfaceType)
		return ok && k.Name == "string" && iface
	}
	return false
}

// goValue evaluates the expected value of a test row as Go stores it in an
// interface{}: untyped integer constants become int, untyped floating-point
// constants float64.
func goValue(e goast.Expr) any {
	switch e := e.(type) {
	case *goast.BasicLit:
		switch e.Kind {
		case gotoken.INT:
			n, err := strconv.ParseInt(e.Value, 0, 64)
			if err != nil {
				panic(err)
			}
			return int(n)
		case gotoken.FLOAT:
			f, err := strconv.ParseFloat(e.Value, 64)
			if err != nil {
				panic(err)
			}
			return f
		case gotoken.STRING:
			s, _ := strValue(e)
			return s
		}
	case *goast.Ident:
		switch e.Name {
		case "true":
			return true
		case "false":
			return false
		case "nil":
			return nil
		}
	case *goast.ParenExpr:
		return goValue(e.X)
	case *goast.UnaryExpr:
		if lit, ok := e.X.(*goast.BasicLit); ok && e.Op == gotoken.SUB && lit.Kind == gotoken.INT {
			n, err := strconv.ParseInt("-"+lit.Value, 0, 64)
			if err != nil {
				panic(err)
			}
			return int(n)
		}
		switch v := goValue(e.X).(type) {
		case int:
			if e.Op == gotoken.SUB {
				return -v
			}
			return v
		case float64:
			if e.Op == gotoken.SUB {
				return -v
			}
			return v
		}
	case *goast.CallExpr:
		switch {
		case isSelector(e.Fun, "math", "Inf"):
			n := goValue(e.Args[0]).(int)
			return math.Inf(n)
		case isSelector(e.Fun, "time", "Date"):
			var a [7]int
			for i := range a {
				a[i] = goInt(e.Args[i])
			}
			if !isSelector(e.Args[7], "time", "UTC") {
				panic("time.Date: location")
			}
			return time.Date(a[0], time.Month(a[1]), a[2], a[3], a[4], a[5], a[6], time.UTC)
		}
		if id, ok := e.Fun.(*goast.Ident); ok && id.Name == "float64" {
			switch v := goValue(e.Args[0]).(type) {
			case int:
				return float64(v)
			case float64:
				return v
			}
		}
	case *goast.CompositeLit:
		switch t := e.Type.(type) {
		case *goast.ArrayType:
			if _, ok := t.Elt.(*goast.InterfaceType); ok {
				list := []any{}
				for _, elt := range e.Elts {
					list = append(list, goValue(elt))
				}
				return list
			}
		case *goast.MapType, *goast.Ident:
			if isStringMapType(t) {
				m := map[string]any{}
				for _, elt := range e.Elts {
					kv := elt.(*goast.KeyValueExpr)
					k, ok := strValue(kv.Key)
					if !ok {
						panic("map key")
					}
					m[k] = goValue(kv.Value)
				}
				return m
			}
			if mt, ok := t.(*goast.MapType); ok {
				_, ik := mt.Key.(*goast.InterfaceType)
				_, iv := mt.Value.(*goast.InterfaceType)
				if ik && iv {
					m := map[any]any{}
					for _, elt := range e.Elts {
						kv := elt.(*goast.KeyValueExpr)
						m[goValue(kv.Key)] = goValue(kv.Value)
					}
					return m
				}
			}
		}
	}
	panic(fmt.Sprintf("goValue: unsupported %T %#v", e, e))
}

// goInt evaluates an integer argument; a floating-point constant like .12e9
// must be integral.
func goInt(e goast.Expr) int {
	switch v := goValue(e).(type) {
	case int:
		return v
	case float64:
		if v != math.Trunc(v) {
			panic("goInt: not integral")
		}
		return int(v)
	}
	panic("goInt: not a number")
}
