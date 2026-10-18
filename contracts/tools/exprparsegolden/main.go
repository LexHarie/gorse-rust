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

// Command exprparsegolden writes what the expr-lang parser (the version
// go.mod pins, v1.17.8) does with a corpus of inputs, for the tests of the
// Rust port in crates/gorse-expr:
//
//	go run ./contracts/tools/exprparsegolden -out-dir crates/gorse-expr/tests/data
//
// Gorse calls parser.Parse to validate the item_expr config fields
// (config/config.go:970-977), so the verdict and error of Parse are the
// contract; the tree is recorded too, for the checker and compiler that
// will be built on it.
//
// parser_golden.json holds, in full:
//
//   - cases: every string literal of expr-lang's parser, lexer, AST printer
//     and parser benchmark tests (with the lexer error table split the way
//     its test splits it), every item_expr value in Gorse's tests and config
//     files, inputs written for the parser's corners (quirkInputs), and the
//     expressions of expr-lang's docs and examples. Each case
//     has the error of parser.Parse (or the panic), and on success
//     ast.Dump of the tree, Node.String and the nodes with their locations
//     in ast.Walk order; and what lexer.Lex gives for the same input.
//   - config: parser.ParseWithConfig of the cases with configs that set the
//     node budget, disable the if operator, disable builtins, or define
//     functions and environment names that override builtins.
//   - synthetic: inputs built by repeating a pattern (deep nesting, long
//     chains around the node budget, long tokens), named by family and count
//     so the Rust test builds the same string.
//   - quote: Token.String of Bytes tokens holding arbitrary bytes, which is
//     strconv.Quote, the quoting of every error message and printed string.
//   - unicode: the code points unicode.IsLetter, unicode.IsDigit,
//     unicode.IsSpace and strconv.IsPrint accept, as ranges.
//   - builtins, default_max_nodes: builtin.Names and conf.DefaultMaxNodes.
//
// parser_bulk_golden.json holds the large corpus compactly: expr-lang's
// generated and fuzz corpora, its crowdsec expressions, the string literals
// of its other tests, every string of up to two (and some of three)
// characters over an alphabet of interesting characters, single characters
// across Unicode, seeded random token sequences and seeded random edits of
// corpus expressions. Each entry is [source, input, accepted, digest], the
// digest being the first 8 bytes of the SHA-256 of everything a full case
// records (see digest). Run with -explain to print the full case of one
// input.
//
// parser_go_tests.json holds the test tables of expr-lang's parser, lexer
// and printer tests (TestParse, TestParse_error, TestLex, TestPrint, ...),
// read from their source (see tables.go), which the Rust port runs as its
// own tests.
//
// Inputs that are not valid UTF-8 cannot be passed to the Rust parser (it
// takes &str) and are counted, per source, under skipped_invalid_utf8.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	goast "go/ast"
	goparser "go/parser"
	gotoken "go/token"
	"math/rand/v2"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/expr-lang/expr/ast"
	"github.com/expr-lang/expr/builtin"
	"github.com/expr-lang/expr/conf"
	"github.com/expr-lang/expr/file"
	"github.com/expr-lang/expr/parser"
	"github.com/expr-lang/expr/parser/lexer"
)

const exprModule = "github.com/expr-lang/expr"

// Result is what parser.Parse gives: the error text, or the panic, or the
// tree as ast.Dump, Node.String and the walk (the nodes in ast.Walk order
// with their locations).
type Result struct {
	Error  *string `json:"error"`
	Panic  *string `json:"panic"`
	Dump   *string `json:"dump"`
	String *string `json:"string"`
	Walk   *string `json:"walk"`
}

// Token is a lexer token; Value is the hex of its bytes (a Bytes token may
// hold any bytes) and String is Token.String.
type Token struct {
	Kind   string `json:"kind"`
	Value  string `json:"value"`
	From   int    `json:"from"`
	To     int    `json:"to"`
	String string `json:"string"`
}

// Lex is what lexer.Lex gives.
type Lex struct {
	Error  *string `json:"error"`
	Tokens []Token `json:"tokens"`
}

type Case struct {
	Source string `json:"source"`
	Input  string `json:"input"`
	Result
	Lex Lex `json:"lex"`
}

// Config is a conf.Config made by conf.CreateNew (or the zero Config when
// Zero is set) with these fields changed.
type Config struct {
	Zero              bool     `json:"zero"`
	MaxNodes          uint     `json:"max_nodes"`
	DisableIfOperator bool     `json:"disable_if_operator"`
	Disabled          []string `json:"disabled"`
	Functions         []string `json:"functions"`
	Env               []string `json:"env"`
}

// ConfigCase is [config index, input, accepted, digest of the Result].
type ConfigCase [4]any

// Synthetic is the input family repeated Count times (see synthetic).
type Synthetic struct {
	Family string `json:"family"`
	Count  int    `json:"count"`
	// The first line of the error: the snippet repeats the whole input.
	ErrorLine *string `json:"error_line"`
	Panic     *string `json:"panic"`
	// Digest of the Result without Dump (whose indentation grows with the
	// square of the depth) and of the lexer's tokens.
	Digest string `json:"digest"`
}

type Golden struct {
	Builtins        []string              `json:"builtins"`
	DefaultMaxNodes uint                  `json:"default_max_nodes"`
	SkippedInvalid  map[string]int        `json:"skipped_invalid_utf8"`
	Unicode         map[string][][2]int32 `json:"unicode"`
	Quote           [][2]string           `json:"quote"`
	Cases           []Case                `json:"cases"`
	Configs         []Config              `json:"configs"`
	Config          []ConfigCase          `json:"config"`
	Synthetic       []Synthetic           `json:"synthetic"`
}

// BulkEntry is [source index, input, accepted, digest].
type BulkEntry [4]any

type Bulk struct {
	Sources        []string       `json:"sources"`
	SkippedInvalid map[string]int `json:"skipped_invalid_utf8"`
	Entries        []BulkEntry    `json:"entries"`
}

func str(s string) *string { return &s }

type walker struct{ out strings.Builder }

func (w *walker) Visit(node *ast.Node) {
	loc := (*node).Location()
	fmt.Fprintf(&w.out, "%s %d:%d\n", strings.TrimPrefix(fmt.Sprintf("%T", *node), "*ast."), loc.From, loc.To)
}

func result(input string, config *conf.Config, withDump bool) (r Result) {
	defer func() {
		if p := recover(); p != nil {
			r = Result{Panic: str(fmt.Sprint(p))}
		}
	}()
	var tree *parser.Tree
	var err error
	if config == nil {
		tree, err = parser.Parse(input)
	} else {
		tree, err = parser.ParseWithConfig(input, config)
	}
	if err != nil {
		return Result{Error: str(err.Error())}
	}
	if withDump {
		r.Dump = str(ast.Dump(tree.Node))
	}
	r.String = str(tree.Node.String())
	var w walker
	ast.Walk(&tree.Node, &w)
	r.Walk = str(w.out.String())
	return r
}

func lex(input string) (l Lex) {
	tokens, err := lexer.Lex(file.NewSource(input))
	if err != nil {
		return Lex{Error: str(err.Error())}
	}
	l.Tokens = []Token{}
	for _, t := range tokens {
		l.Tokens = append(l.Tokens, Token{
			Kind:   string(t.Kind),
			Value:  hex.EncodeToString([]byte(t.Value)),
			From:   t.From,
			To:     t.To,
			String: t.String(),
		})
	}
	return l
}

// lexText is the lexer result as the digest sees it.
func lexText(l Lex) *string {
	if l.Error != nil {
		return str("error " + *l.Error)
	}
	var b strings.Builder
	for _, t := range l.Tokens {
		fmt.Fprintf(&b, "%s %d:%d %s %s\n", t.Kind, t.From, t.To, t.Value, t.String)
	}
	return str(b.String())
}

// digest is the first 8 bytes, in hex, of the SHA-256 of the parts, each
// written as "-\n" when absent and "+<byte length>:<text>\n" otherwise.
func digest(parts ...*string) string {
	h := sha256.New()
	for _, p := range parts {
		if p == nil {
			h.Write([]byte("-\n"))
		} else {
			fmt.Fprintf(h, "+%d:%s\n", len(*p), *p)
		}
	}
	return hex.EncodeToString(h.Sum(nil)[:8])
}

func fullCase(source, input string) Case {
	return Case{Source: source, Input: input, Result: result(input, nil, true), Lex: lex(input)}
}

func fullDigest(c Case) string {
	return digest(c.Error, c.Panic, c.Dump, c.String, c.Walk, lexText(c.Lex))
}

// moduleDir finds the expr-lang module source go.mod pins.
func moduleDir() string {
	out, err := exec.Command("go", "list", "-m", "-f", "{{.Version}} {{.Dir}}", exprModule).Output()
	if err != nil {
		panic(err)
	}
	version, dir, ok := strings.Cut(strings.TrimSpace(string(out)), " ")
	if !ok || version != "v1.17.8" || dir == "" {
		panic(fmt.Sprintf("unexpected %s module: %q", exprModule, out))
	}
	return dir
}

// constString evaluates a Go string literal or a concatenation of them.
func constString(e goast.Expr) (string, bool) {
	switch e := e.(type) {
	case *goast.BasicLit:
		if e.Kind != gotoken.STRING {
			return "", false
		}
		s, err := strconv.Unquote(e.Value)
		return s, err == nil
	case *goast.BinaryExpr:
		if e.Op != gotoken.ADD {
			return "", false
		}
		l, ok1 := constString(e.X)
		r, ok2 := constString(e.Y)
		return l + r, ok1 && ok2
	case *goast.ParenExpr:
		return constString(e.X)
	}
	return "", false
}

// goStrings returns every string literal of a Go file and every
// concatenation of literals, in source order; named holds the value of each
// package-level constant that is one.
func goStrings(path string) (all []string, named map[string]string) {
	f, err := goparser.ParseFile(gotoken.NewFileSet(), path, nil, 0)
	if err != nil {
		panic(err)
	}
	named = map[string]string{}
	goast.Inspect(f, func(n goast.Node) bool {
		switch n := n.(type) {
		case *goast.ValueSpec:
			for i, name := range n.Names {
				if i < len(n.Values) {
					if s, ok := constString(n.Values[i]); ok {
						named[name.Name] = s
					}
				}
			}
		case *goast.BinaryExpr:
			if s, ok := constString(n); ok {
				all = append(all, s)
			}
		case *goast.BasicLit:
			if s, ok := constString(n); ok {
				all = append(all, s)
			}
		}
		return true
	})
	return all, named
}

// lexerErrorInputs splits lexer_test.go's errorTests table the way
// TestLex_error does and returns the inputs.
func lexerErrorInputs(table string) []string {
	var inputs []string
	for _, test := range strings.Split(strings.Trim(table, "\n"), "\n\n") {
		input := strings.SplitN(test, "\n", 2)
		if len(input) != 2 {
			panic("lexer error table changed shape")
		}
		inputs = append(inputs, input[0])
	}
	return inputs
}

// markdownBlocks returns the fenced code blocks of a Markdown file that are
// marked expr or not marked at all.
func markdownBlocks(path string) []string {
	var out, block []string
	open, keep := false, false
	for _, line := range lines(path) {
		if !strings.HasPrefix(line, "```") {
			if open {
				block = append(block, line)
			}
			continue
		}
		if open {
			if keep {
				out = append(out, strings.Join(block, "\n"))
			}
			open = false
			continue
		}
		info := strings.TrimSpace(strings.TrimPrefix(line, "```"))
		open, keep, block = true, info == "" || info == "expr", nil
	}
	if open {
		panic("unclosed code block in " + path)
	}
	return out
}

func lines(path string) []string {
	data, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	return strings.Split(strings.TrimRight(string(data), "\n"), "\n")
}

// gorseExpressions are the item_expr values (and the expressions passed to
// expr.Compile) in the Go tests and config files of the pinned tree.
var gorseExpressions = []struct{ source, input string }{
	{"config/config.toml", "item.Comment"},
	{"config/config.toml", "count(feedback, .FeedbackType == 'star')"},
	{"config/config.toml", "(now() - item.Timestamp).Hours() < 168"},
	{"config/config.toml", "item.Labels.embedding"},
	{"client/config.toml", "len(feedback)"},
	{"config/config_test.go", "a"},
	{"config/config_test.go", "b"},
	{"config/config_test.go", "item.Labels.description"},
	{"config/config_test.go", "item.Labels."},
	{"master/tasks_test.go", "item.Labels"},
	{"master/tasks_test.go", "user.Labels"},
	{"master/tasks_test.go", "item.Timestamp.Unix()"},
	{"master/tasks_test.go", "1"},
	{"logics/user_to_user_test.go", "user.Labels.description"},
	{"logics/user_to_user_test.go", "user.Labels.embedding"},
	{"logics/non_personalized_test.go", "!item.IsHidden"},
	{"storage/data/database_test.go", "item.ItemId"},
	{"storage/data/database_test.go", "item.Categories"},
	{"storage/data/database_test.go", "item.Labels.brand"},
	{"contracts/tools/schemadump/main.go", "item.Labels.title"},
	{"empty value, which the validator accepts without parsing", ""},
}

// quirkInputs exercise the corners of the lexer and parser that the
// collected inputs may miss: escapes (Go's int32 rune overflow in \U,
// surrogates, the escapes the lexer and the unescaper disagree on), newline
// normalization, if/else and ternary forms, comparison chains, "not"
// operators, postfix after literals, optional chaining, coalescing, number
// edge cases, predicates and pipes, and error positions.
var quirkInputs = []string{
	`"\UFFFFFFFF"`, `"\U80000041"`, `"\U7FFFFFFF"`, `"\U0010FFFF"`, `"\U00110000"`,
	`"\uD800"`, `"\u{D800}"`, `"\u{10FFFF}"`, `"\u{110000}"`, `"\u{}"`, `"\u{1234567}"`, `"\u{41"`,
	`'\"'`, `"\'"`, `"\400"`, `"\777"`, `"\?"`, `"\` + "`" + `"`, `'\x41\101'`, `b"\xff\377"`, `b"\u0041"`,
	`b'é'`, `b"\x00"`, `"\x"`, `"\xZZ"`, `"\q"`, "\"a\r\nb\rc\"", "`a\r\nb`", "'a\r\n'",
	`if a { 1 } else { 2 }`, `if a { 1 } else if b { 2 } else { 3 }`, `(if a { 1 } else { 2 }).b`,
	`if a { 1 } else { 2 } + 1`, `{a: 1, "b": 2, 3: 4, (c): 5}`, `{a: 1,}`, `{a: 1, , b: 2}`,
	`a ? b : c`, `a ?: b`, `a ?: b ?: c`, `a ? b : c ? d : e`, `a < b < c <= d`, `a < b > c`,
	`a not in b`, `a not "in" b`, `a + b not in c`, `a not b`, `not a in b`, `a not matches b`,
	`true.foo`, `false[0]`, `nil[0]`, `1.foo`, `1[0]`, `"s".len()`, `foo.not()`, `foo.if`, `foo.and.or`,
	`foo.matches(1)`, `foo?.[0]`, `foo?.bar?.[0].baz()`, `foo?.bar.baz?.qux`, `a?.b()`, `a.b.c()`,
	`a ?? b ?? c`, `a ?? (b || c)`, `a ?? b + c`, `a + b ?? c`,
	`1..2`, `1...2`, `0x`, `0x_1`, `1_0`, `08`, `0b2`, `0o8`, `1e400`, `1e-400`,
	`1.7976931348623157e308`, `1.7976931348623159e308`, `9223372036854775807`, `9223372036854775808`,
	`-9223372036854775808`, `0x7fffffffffffffff`, `0x8000000000000000`, `0o777777777777777777777`,
	`0b1111111111111111111111111111111111111111111111111111111111111111`, `.5`, `..5`, `5.`, `1.e5`, `1e+5`,
	`filter(a, # > 0)`, `map(a, #index)`, `map(a, .b)`, `sortBy(a, .b, "desc")`, `sortBy(a, .b,)`,
	`reduce(a, #acc + #, 0)`, `a | map(# * 2) | sum()`, `a | sortBy(.b)`, `a | sortBy()`, `count(a)`,
	`count(a,)`, `all(a)`, `all(a, {# > 1; # < 2})`, `all(a, # > 1; # < 2)`, `::len(a)`, `::foo(a)`,
	`let x = 1; x + 1`, `let x = 1; let y = x; y`, `let x = 1`, `let 1 = x; x`, `a; b; c`, `a;`, `a;;`,
	`##`, `#a`, `.a`, `filter(a, ##)`, "a\n+\n", "a +\n\n", "\t\tfoo bar", `/* unterminated`, "// comment\n1",
	`a /* c */ + b`, "\"literal\nnot terminated\"", `é + ü`, `♥`, `a + 😀`, `[1, 2, 3,]`, `[1,,2]`, `f(1,)`,
	`f(,)`, `a[1:2]`, `a[:2]`, `a[1:]`, `a[:]`, `a?.[1:2]`, `a[1:2:3]`, `len`, `len()`, `now()`,
	`-a ** b`, `a ** b ** c`, `(a ** b) ** c`, `!a && b`, `not (a and b) or c`, `a in b..c`,
	`a matches "x" and b`, "\"a\rb\"", "'a\r'", `count(, 1)`, `count(,"\q")`,
}

type corpus struct {
	seen    map[string]bool
	skipped map[string]int
}

// add reports whether input is new and valid UTF-8.
func (c *corpus) add(source, input string) bool {
	if !utf8.ValidString(input) {
		c.skipped[source]++
		return false
	}
	if c.seen[input] {
		return false
	}
	c.seen[input] = true
	return true
}

// alphabet holds characters that start or break tokens, letters and digits
// of other scripts, spaces Go counts and characters it rejects.
var alphabet = []string{
	"a", "b", "B", "x", "e", "E", "o", "_", "$", "0", "1", "7", "9",
	".", ",", ":", ";", "?", "!", "|", "&", "=", "<", ">", "+", "-", "*",
	"/", "%", "^", "#", "(", ")", "[", "]", "{", "}", "'", "\"", "`", "\\",
	" ", "\n", "\t", "\r", "\v", "\f", "~", "@",
	"\x00", "\x7f",
	string(rune(0x85)), string(rune(0xa0)), string(rune(0xad)), string(rune(0xe9)),
	string(rune(0x301)), string(rune(0x663)), string(rune(0x2028)),
	string(rune(0x2165)), string(rune(0x2665)), string(rune(0x3000)),
	string(rune(0xfeff)), string(rune(0xfffd)), string(rune(0x1f600)),
}

var small = []string{
	"a", "1", ".", ",", ":", "?", "!", "|", "=", "<", "-", "*", "#", "(", ")",
	"[", "]", "{", "}", "'", "`", "\\", " ",
}

// tokens are the pieces random sequences are built from.
var tokens = []string{
	"not", "in", "and", "or", "let", "if", "else", "matches", "contains",
	"startsWith", "endsWith", "true", "false", "nil", "a", "b", "foo", "1",
	"0", "1.5", ".5", "1e3", "0x1F", "0b101", "0o17", "1_000", "99999999999999999999",
	"1e999", "'s'", "\"s\"", "`raw`", "b'x'", "'\\x41'", "'\\u{1F600}'", "(", ")",
	"[", "]", "{", "}", ",", ":", ";", "?", "?.", "??", ".", "..", "::",
	"|", "#", "#index", "#acc", "+", "-", "*", "/", "%", "**", "^", "==",
	"!=", "<", "<=", ">", ">=", "!", "&&", "||", "=", "len", "all", "map",
	"filter", "count", "sum", "reduce", "sortBy", "find", "now", "$env",
	"//c\n", "/*c*/",
}

func randomTokens(rng *rand.Rand) string {
	n := 1 + rng.IntN(8)
	var b strings.Builder
	for i := 0; i < n; i++ {
		if i > 0 && rng.IntN(3) > 0 {
			b.WriteByte(' ')
		}
		b.WriteString(tokens[rng.IntN(len(tokens))])
	}
	return b.String()
}

// mutate applies one to three random edits to the runes of s.
func mutate(rng *rand.Rand, s string) string {
	r := []rune(s)
	for edits := 1 + rng.IntN(3); edits > 0; edits-- {
		var piece []rune
		if rng.IntN(2) == 0 {
			piece = []rune(alphabet[rng.IntN(len(alphabet))])
		} else {
			piece = []rune(tokens[rng.IntN(len(tokens))])
		}
		switch at := rng.IntN(len(r) + 1); rng.IntN(4) {
		case 0: // insert
			r = append(r[:at], append(piece, r[at:]...)...)
		case 1: // delete
			if at < len(r) {
				r = append(r[:at], r[at+1:]...)
			}
		case 2: // replace
			if at < len(r) {
				r = append(r[:at], append(piece, r[at+1:]...)...)
			}
		default: // truncate
			r = r[:at]
		}
	}
	return string(r)
}

// synthetic builds the input of a family repeated n times.
func synthetic(family string, n int) string {
	switch family {
	case "paren":
		return strings.Repeat("(", n) + "a" + strings.Repeat(")", n)
	case "unary":
		return strings.Repeat("-", n) + "1"
	case "not":
		return strings.Repeat("not ", n) + "a"
	case "plus":
		return "1" + strings.Repeat("+1", n)
	case "array":
		return "[" + strings.TrimSuffix(strings.Repeat("1,", n), ",") + "]"
	case "nested_array":
		return strings.Repeat("[", n) + strings.Repeat("]", n)
	case "member":
		return "a" + strings.Repeat(".b", n)
	case "call":
		return strings.Repeat("f(", n) + "1" + strings.Repeat(")", n)
	case "ternary":
		return strings.Repeat("a?b:", n) + "c"
	case "sequence":
		return strings.Repeat("1;", n) + "1"
	case "pipe":
		return "a" + strings.Repeat(" | f()", n)
	case "identifier":
		return strings.Repeat("a", n)
	case "string":
		return "'" + strings.Repeat("x", n) + "'"
	case "comment":
		return "/*" + strings.Repeat("*", n) + "*/1"
	case "repeat_plus":
		// TestNodeBudgetDisabled
		return strings.Repeat("a + ", n) + "b"
	}
	panic("unknown family " + family)
}

var syntheticCases = []struct {
	family string
	counts []int
}{
	{"paren", []int{1, 100, 10000, 200000}},
	{"unary", []int{9998, 9999, 10000}},
	{"not", []int{9998, 9999, 10000}},
	{"plus", []int{4999, 5000}},
	{"array", []int{9999, 10000}},
	{"nested_array", []int{10000, 10001}},
	{"member", []int{4999, 5000}},
	{"call", []int{4999, 5000}},
	{"ternary", []int{3333, 3334}},
	{"sequence", []int{9998, 9999}},
	{"pipe", []int{4999, 5000}},
	{"identifier", []int{100000}},
	{"string", []int{100000}},
	{"comment", []int{100000}},
	{"repeat_plus", []int{1000}},
}

var configs = []Config{
	{Zero: true},
	{MaxNodes: 0},
	{MaxNodes: 1},
	{MaxNodes: 3},
	{MaxNodes: 5},
	{MaxNodes: 10000},
	{MaxNodes: 10000, DisableIfOperator: true},
	{MaxNodes: 10000, Disabled: []string{"all", "count", "len", "map", "now", "split", "trim"}},
	{MaxNodes: 10000, Functions: []string{"all", "count", "foo", "len", "map", "sortBy"}},
	{MaxNodes: 10000, Env: []string{"a", "filter", "if", "len", "now", "reduce", "sum"}},
	{MaxNodes: 10000, DisableIfOperator: true, Disabled: []string{"len"}, Functions: []string{"map"}, Env: []string{"if"}},
}

func makeConfig(c Config) *conf.Config {
	if c.Zero {
		return &conf.Config{}
	}
	config := conf.CreateNew()
	if len(c.Env) > 0 {
		env := map[string]any{}
		for _, name := range c.Env {
			env[name] = 0
		}
		config.WithEnv(env)
	}
	config.MaxNodes = c.MaxNodes
	config.DisableIfOperator = c.DisableIfOperator
	for _, name := range c.Disabled {
		config.Disabled[name] = true
	}
	for _, name := range c.Functions {
		config.Functions[name] = &builtin.Function{Name: name}
	}
	return config
}

func ranges(f func(rune) bool) [][2]int32 {
	var out [][2]int32
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if !f(r) {
			continue
		}
		if n := len(out); n > 0 && out[n-1][1]+1 == r {
			out[n-1][1] = r
		} else {
			out = append(out, [2]int32{r, r})
		}
	}
	return out
}

func quoteCases(rng *rand.Rand) [][2]string {
	var out [][2]string
	add := func(b []byte) {
		t := lexer.Token{Kind: lexer.Bytes, Value: string(b)}
		out = append(out, [2]string{hex.EncodeToString(b), t.String()})
	}
	for b := 0; b < 256; b++ {
		add([]byte{byte(b)})
	}
	// Every code point at the edge of a print range, and a spread of others.
	edges := map[rune]bool{}
	for _, r := range ranges(strconv.IsPrint) {
		edges[r[0]-1], edges[r[0]], edges[r[1]], edges[r[1]+1] = true, true, true, true
	}
	for r := rune(0x100); r <= unicode.MaxRune; r += 0xfff {
		edges[r] = true
	}
	var points []rune
	for r := range edges {
		if r >= 0 && r <= unicode.MaxRune && utf8.ValidRune(r) {
			points = append(points, r)
		}
	}
	sort.Slice(points, func(i, j int) bool { return points[i] < points[j] })
	for _, r := range points {
		add([]byte(string(r)))
	}
	for i := 0; i < 1000; i++ {
		b := make([]byte, 1+rng.IntN(8))
		for j := range b {
			switch rng.IntN(3) {
			case 0:
				b[j] = byte(0x20 + rng.IntN(0x5f))
			case 1:
				b[j] = byte(0x80 + rng.IntN(0x80))
			default:
				b[j] = byte(rng.IntN(256))
			}
		}
		add(b)
	}
	for i := 0; i < 300; i++ {
		var b []byte
		for j := 1 + rng.IntN(4); j > 0; j-- {
			b = utf8.AppendRune(b, rune(rng.IntN(unicode.MaxRune+1)))
		}
		add(b)
	}
	return out
}

func main() {
	outDir := flag.String("out-dir", "", "output directory")
	explain := flag.String("explain", "", "print the full case of this input and exit")
	flag.Parse()
	if *explain != "" {
		c := fullCase("explain", *explain)
		data, err := json.MarshalIndent(c, "", "  ")
		if err != nil {
			panic(err)
		}
		fmt.Printf("%s\ndigest %s\n", data, fullDigest(c))
		return
	}
	dir := moduleDir()
	rng := rand.New(rand.NewPCG(20261017, 19))

	// Curated cases, in full.
	full := &corpus{seen: map[string]bool{}, skipped: map[string]int{}}
	var cases []Case
	addFull := func(source, input string) {
		if full.add(source, input) {
			cases = append(cases, fullCase(source, input))
		}
	}
	for _, e := range gorseExpressions {
		addFull(e.source, e.input)
	}
	for _, input := range quirkInputs {
		addFull("quirks", input)
	}
	curated := []string{"parser/parser_test.go", "parser/lexer/lexer_test.go", "ast/print_test.go", "parser/bench_test.go"}
	for _, name := range curated {
		all, named := goStrings(filepath.Join(dir, name))
		if table, ok := named["errorTests"]; ok {
			for _, input := range lexerErrorInputs(table) {
				addFull(name, input)
			}
		}
		for _, s := range all {
			addFull(name, s)
		}
	}
	for _, name := range []string{"docs/configuration.md", "docs/environment.md", "docs/functions.md",
		"docs/getting-started.md", "docs/language-definition.md", "docs/patch.md", "docs/visitor.md",
		"testdata/examples.md"} {
		for _, block := range markdownBlocks(filepath.Join(dir, name)) {
			addFull(name, block)
		}
	}
	for _, input := range lines(filepath.Join(dir, "testdata/crash.txt")) {
		addFull("testdata/crash.txt", input)
	}

	var configCases []ConfigCase
	for i, c := range configs {
		config := makeConfig(c)
		for _, fc := range cases {
			if strings.HasPrefix(fc.Source, "docs/") || strings.HasPrefix(fc.Source, "testdata/") {
				continue
			}
			r := result(fc.Input, config, true)
			configCases = append(configCases, ConfigCase{i, fc.Input, r.Error == nil && r.Panic == nil,
				digest(r.Error, r.Panic, r.Dump, r.String, r.Walk)})
		}
	}

	var synth []Synthetic
	for _, s := range syntheticCases {
		for _, n := range s.counts {
			input := synthetic(s.family, n)
			r := result(input, nil, false)
			var line *string
			if r.Error != nil {
				first, _, _ := strings.Cut(*r.Error, "\n")
				line = &first
			}
			synth = append(synth, Synthetic{Family: s.family, Count: n, ErrorLine: line, Panic: r.Panic,
				Digest: digest(r.Error, r.Panic, r.String, r.Walk, lexText(lex(input)))})
		}
	}

	g := Golden{
		Builtins:        builtin.Names,
		DefaultMaxNodes: conf.DefaultMaxNodes,
		SkippedInvalid:  full.skipped,
		Unicode: map[string][][2]int32{
			"letter": ranges(unicode.IsLetter),
			"digit":  ranges(unicode.IsDigit),
			"space":  ranges(unicode.IsSpace),
			"print":  ranges(strconv.IsPrint),
		},
		Quote:     quoteCases(rng),
		Cases:     cases,
		Configs:   configs,
		Config:    configCases,
		Synthetic: synth,
	}
	write(filepath.Join(*outDir, "parser_golden.json"), g,
		[]string{"builtins", "default_max_nodes", "skipped_invalid_utf8", "unicode", "quote", "cases", "configs", "config", "synthetic"})

	write(filepath.Join(*outDir, "parser_go_tests.json"), goTests(dir),
		[]string{"lex", "lex_error", "node_budget", "optional_chaining", "parse", "parse_error", "print", "skipped_invalid_utf8"})

	// The bulk corpus: everything the curated set does not already hold.
	b := Bulk{SkippedInvalid: map[string]int{}}
	bulk := &corpus{seen: full.seen, skipped: b.SkippedInvalid}
	source := func(name string) int {
		b.Sources = append(b.Sources, name)
		return len(b.Sources) - 1
	}
	addBulk := func(src int, input string) {
		if bulk.add(b.Sources[src], input) {
			c := fullCase("", input)
			if c.Panic != nil {
				// The entry format has no room for a panic; none occurs today.
				panic(fmt.Sprintf("the parser panicked on %q: %s", input, *c.Panic))
			}
			b.Entries = append(b.Entries, BulkEntry{src, input, c.Error == nil && c.Panic == nil, fullDigest(c)})
		}
	}
	src := source("testdata/generated.txt")
	generated := lines(filepath.Join(dir, "testdata/generated.txt"))
	for _, input := range generated {
		addBulk(src, input)
	}
	src = source("test/fuzz/fuzz_corpus.txt")
	for _, input := range lines(filepath.Join(dir, "test/fuzz/fuzz_corpus.txt")) {
		addBulk(src, input)
	}
	src = source("testdata/crowdsec.json")
	data, err := os.ReadFile(filepath.Join(dir, "testdata/crowdsec.json"))
	if err != nil {
		panic(err)
	}
	var crowdsec []string
	if err = json.Unmarshal(data, &crowdsec); err != nil {
		panic(err)
	}
	for _, input := range crowdsec {
		addBulk(src, input)
	}
	src = source("string literals of the other expr-lang tests")
	var testFiles []string
	err = filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(dir, path)
		if d.IsDir() && (rel == "internal" || rel == "docgen") {
			return filepath.SkipDir
		}
		if strings.HasSuffix(path, "_test.go") {
			testFiles = append(testFiles, rel)
		}
		return nil
	})
	if err != nil {
		panic(err)
	}
	sort.Strings(testFiles)
	for _, name := range testFiles {
		all, _ := goStrings(filepath.Join(dir, name))
		for _, s := range all {
			addBulk(src, s)
		}
	}
	src = source("strings of one and two characters")
	for _, x := range alphabet {
		addBulk(src, x)
	}
	for _, x := range alphabet {
		for _, y := range alphabet {
			addBulk(src, x+y)
		}
	}
	src = source("strings of three characters")
	for _, x := range small {
		for _, y := range small {
			for _, z := range small {
				addBulk(src, x+y+z)
			}
		}
	}
	src = source("single characters across Unicode")
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if utf8.ValidRune(r) && (r < 0x300 || r%997 == 0) {
			addBulk(src, string(r))
		}
	}
	src = source("random token sequences")
	for i := 0; i < 8000; i++ {
		addBulk(src, randomTokens(rng))
	}
	src = source("random edits of corpus expressions")
	for i := 0; i < 8000; i++ {
		addBulk(src, mutate(rng, generated[rng.IntN(len(generated))]))
	}
	write(filepath.Join(*outDir, "parser_bulk_golden.json"), b, []string{"sources", "skipped_invalid_utf8", "entries"})
}

// write writes v as JSON with one array element per line, the keys in the
// given order.
func write(path string, v any, keys []string) {
	var raw map[string]json.RawMessage
	data, err := json.Marshal(v)
	if err != nil {
		panic(err)
	}
	if err = json.Unmarshal(data, &raw); err != nil {
		panic(err)
	}
	if len(keys) != len(raw) {
		panic("golden section list is out of date")
	}
	var b strings.Builder
	b.WriteString("{\n")
	for i, key := range keys {
		fmt.Fprintf(&b, "%q: ", key)
		var items []json.RawMessage
		if json.Unmarshal(raw[key], &items) == nil && len(items) > 0 {
			b.WriteString("[\n")
			for j, item := range items {
				b.Write(item)
				if j+1 < len(items) {
					b.WriteByte(',')
				}
				b.WriteByte('\n')
			}
			b.WriteString("]")
		} else {
			b.Write(raw[key])
		}
		if i+1 < len(keys) {
			b.WriteByte(',')
		}
		b.WriteByte('\n')
	}
	b.WriteString("}\n")
	if err = os.WriteFile(path, []byte(b.String()), 0o644); err != nil {
		panic(err)
	}
}
